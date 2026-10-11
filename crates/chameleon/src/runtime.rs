//! The event loop: terminal input, host feeds and script results flow into
//! the app; its effects become processes.

use std::collections::HashMap;
use std::io::Stdout;
use std::sync::Arc;
use std::time::Duration;

use acc_core::Exec;
use chameleon_app::config::{Config, HostConfig};
use chameleon_app::keymap::{Key, KeyCode};
use chameleon_app::{App, AppEvent, Effect, ExecTag, HostId, SessionKey, now_ms};
use crossterm::event::{
    Event as CEvent, KeyCode as CKey, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::hooks::{self, HookRun};
use crate::host::feed::Feed;
use crate::host::{self, Host};

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub struct Runtime {
    app: App,
    hosts: HashMap<HostId, Arc<dyn Host>>,
    feeds: HashMap<HostId, Feed>,
    inputs: HashMap<SessionKey, UnboundedSender<Exec>>,
    tx: UnboundedSender<AppEvent>,
    rx: UnboundedReceiver<AppEvent>,
    tick: Duration,
    exec_timeout: Duration,
}

fn make_host(h: &HostConfig) -> Arc<dyn Host> {
    match &h.ssh {
        None => Arc::new(host::local::LocalHost::new(&h.name)),
        Some(dest) => Arc::new(host::ssh::SshHost::new(
            &h.name,
            dest,
            h.ssh_args.clone(),
            h.connect_timeout_s,
        )),
    }
}

impl Runtime {
    pub fn new(cfg: Config, client_tag: String) -> Self {
        let (tx, rx) = unbounded_channel();
        let tick = Duration::from_millis(cfg.general.tick_ms.clamp(50, 5000));
        let exec_timeout = Duration::from_secs(cfg.general.exec_timeout_s.max(1));
        let mut hosts: HashMap<HostId, Arc<dyn Host>> = HashMap::new();
        let mut feeds = HashMap::new();
        for h in cfg.hosts() {
            let host = make_host(&h);
            feeds.insert(h.name.clone(), Feed::new(host.clone(), tx.clone()));
            hosts.insert(h.name.clone(), host);
        }
        Self {
            app: App::new(cfg, client_tag),
            hosts,
            feeds,
            inputs: HashMap::new(),
            tx,
            rx,
            tick,
            exec_timeout,
        }
    }

    pub async fn run(mut self, terminal: &mut Term) -> std::io::Result<()> {
        self.spawn_input_thread();
        let size = terminal.size()?;
        self.app.handle(
            AppEvent::Resize {
                width: size.width,
                height: size.height,
            },
            now_ms(),
        );
        let effects = self.app.start(now_ms());
        if self.execute(effects, terminal).await {
            return Ok(());
        }
        self.render(terminal)?;
        let mut ticker = tokio::time::interval(self.tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let ev = tokio::select! {
                ev = self.rx.recv() => match ev {
                    Some(ev) => ev,
                    None => break,
                },
                _ = ticker.tick() => AppEvent::Tick,
            };
            let mut quit = self.step(ev, terminal).await;
            // Drain what is already queued before painting.
            let mut n = 0;
            while !quit && n < 256 {
                match self.rx.try_recv() {
                    Ok(ev) => {
                        quit = self.step(ev, terminal).await;
                        n += 1;
                    }
                    Err(_) => break,
                }
            }
            if quit {
                break;
            }
            if self.app.take_dirty() {
                self.render(terminal)?;
            }
        }
        for f in self.feeds.values() {
            f.close().await;
        }
        Ok(())
    }

    async fn step(&mut self, ev: AppEvent, terminal: &mut Term) -> bool {
        let effects = self.app.handle(ev, now_ms());
        self.execute(effects, terminal).await
    }

    fn render(&mut self, terminal: &mut Term) -> std::io::Result<()> {
        let screen = self.app.screen();
        let theme = *self.app.theme();
        terminal.draw(|f| crate::ui::render::draw(f, &screen, &theme))?;
        Ok(())
    }

    /// Carries out effects; true means quit.
    async fn execute(&mut self, effects: Vec<Effect>, terminal: &mut Term) -> bool {
        for e in effects {
            match e {
                Effect::Quit => return true,
                Effect::Run {
                    host,
                    exec,
                    tag,
                    timeout_ms,
                } => {
                    let Some(h) = self.hosts.get(&host).cloned() else {
                        continue;
                    };
                    let tx = self.tx.clone();
                    let timeout = Duration::from_millis(timeout_ms.max(1000));
                    tokio::spawn(async move {
                        let result = host::run(h.as_ref(), &exec, timeout).await;
                        let _ = tx.send(AppEvent::ExecDone { host, tag, result });
                    });
                }
                Effect::Input { host, session, exec } => {
                    let key = SessionKey::new(&host, &session);
                    let queue = self.inputs.entry(key).or_insert_with(|| {
                        let (qtx, mut qrx) = unbounded_channel::<Exec>();
                        let Some(h) = self.hosts.get(&host).cloned() else {
                            return qtx;
                        };
                        let tx = self.tx.clone();
                        let timeout = self.exec_timeout;
                        let (host, session) = (host.clone(), session.clone());
                        tokio::spawn(async move {
                            while let Some(exec) = qrx.recv().await {
                                let result = host::run(h.as_ref(), &exec, timeout).await;
                                let _ = tx.send(AppEvent::ExecDone {
                                    host: host.clone(),
                                    tag: ExecTag::Input(session.clone()),
                                    result,
                                });
                            }
                        });
                        qtx
                    });
                    let _ = queue.send(exec);
                }
                Effect::Follow { host, entry } => {
                    if let Some(f) = self.feeds.get(&host) {
                        f.follow(entry);
                    }
                }
                Effect::Unfollow { host, key } => {
                    if let Some(f) = self.feeds.get(&host) {
                        f.unfollow(&key);
                    }
                }
                Effect::Hook {
                    name,
                    command,
                    payload,
                    env,
                    suspend_tui,
                } => {
                    let run = HookRun {
                        name: name.clone(),
                        command,
                        payload: payload.to_string(),
                        env,
                    };
                    if suspend_tui {
                        let _ = crate::leave(terminal);
                        let (status, stderr) = hooks::run_blocking(&run);
                        let _ = crate::enter(terminal);
                        let _ = self.tx.send(AppEvent::HookDone { name, status, stderr });
                    } else {
                        hooks::spawn(run, self.tx.clone());
                    }
                }
            }
        }
        false
    }

    /// Reads crossterm events on a plain thread; the runtime never blocks on them.
    fn spawn_input_thread(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = crossterm::event::read() {
                let app_ev = match ev {
                    CEvent::Key(k) if k.kind != KeyEventKind::Release => {
                        match convert_key(k.code, k.modifiers) {
                            Some(key) => AppEvent::Key(key),
                            None => continue,
                        }
                    }
                    CEvent::Paste(text) => AppEvent::Paste(text),
                    CEvent::Resize(w, h) => AppEvent::Resize { width: w, height: h },
                    CEvent::Mouse(m) => {
                        let kind = match m.kind {
                            MouseEventKind::Down(MouseButton::Left) => chameleon_app::event::MouseKind::Click,
                            MouseEventKind::ScrollUp => chameleon_app::event::MouseKind::ScrollUp,
                            MouseEventKind::ScrollDown => chameleon_app::event::MouseKind::ScrollDown,
                            _ => continue,
                        };
                        AppEvent::Mouse {
                            x: m.column,
                            y: m.row,
                            kind,
                        }
                    }
                    _ => continue,
                };
                if tx.send(app_ev).is_err() {
                    break;
                }
            }
        });
    }
}

fn convert_key(code: CKey, mods: KeyModifiers) -> Option<Key> {
    let kc = match code {
        CKey::Char(c) => KeyCode::Char(c),
        CKey::Enter => KeyCode::Enter,
        CKey::Esc => KeyCode::Esc,
        CKey::Tab => KeyCode::Tab,
        CKey::BackTab => KeyCode::BackTab,
        CKey::Backspace => KeyCode::Backspace,
        CKey::Delete => KeyCode::Delete,
        CKey::Up => KeyCode::Up,
        CKey::Down => KeyCode::Down,
        CKey::Left => KeyCode::Left,
        CKey::Right => KeyCode::Right,
        CKey::PageUp => KeyCode::PageUp,
        CKey::PageDown => KeyCode::PageDown,
        CKey::Home => KeyCode::Home,
        CKey::End => KeyCode::End,
        CKey::F(n) => KeyCode::F(n),
        _ => return None,
    };
    let mut key = Key::new(kc);
    key.ctrl = mods.contains(KeyModifiers::CONTROL);
    key.alt = mods.contains(KeyModifiers::ALT);
    // Shift only matters for non-character keys; characters already carry it.
    key.shift = mods.contains(KeyModifiers::SHIFT) && !matches!(kc, KeyCode::Char(_));
    if let KeyCode::Char(c) = kc
        && key.ctrl
    {
        key.code = KeyCode::Char(c.to_ascii_lowercase());
    }
    Some(key)
}
