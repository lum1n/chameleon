//! chameleon: one terminal UI for every agent harness, on top of ACC.

mod hooks;
mod host;
#[cfg(feature = "lua")]
mod lua;
mod runtime;
mod ui;

use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use chameleon_app::config::Config;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

const USAGE: &str = "\
usage: chameleon [--config PATH] [--lua PATH] [--log PATH]

  --config PATH   TOML config (default: $XDG_CONFIG_HOME/chameleon/config.toml)
  --lua PATH      Lua script (default: init.lua next to the config; needs --features lua)
  --log PATH      append debug logs here (also CHAMELEON_LOG=PATH)
  --version
";

struct Args {
    config: Option<PathBuf>,
    lua: Option<PathBuf>,
    log: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        config: None,
        lua: None,
        log: std::env::var_os("CHAMELEON_LOG").map(PathBuf::from),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" | "-c" => args.config = Some(PathBuf::from(it.next().ok_or("--config needs a path")?)),
            "--lua" => args.lua = Some(PathBuf::from(it.next().ok_or("--lua needs a path")?)),
            "--log" => args.log = Some(PathBuf::from(it.next().ok_or("--log needs a path")?)),
            "--version" | "-V" => {
                println!(
                    "chameleon {} (acc-core {})",
                    env!("CARGO_PKG_VERSION"),
                    acc_core::VERSION
                );
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    Ok(args)
}

fn load_config(path: Option<PathBuf>) -> Result<Config, String> {
    let explicit = path.is_some();
    let Some(path) = path.or_else(Config::default_path) else {
        return Ok(Config::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => Config::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => Ok(Config::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "host".into())
}

/// A tiny file logger, on only when asked.
struct FileLog(std::sync::Mutex<std::fs::File>);

impl log::Log for FileLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        use std::io::Write as _;
        if let Ok(mut f) = self.0.lock() {
            let _ = writeln!(f, "{} {} {}", record.level(), record.target(), record.args());
        }
    }
    fn flush(&self) {}
}

pub fn enter(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> std::io::Result<()> {
    enable_raw_mode()?;
    crossterm::execute!(
        stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    terminal.clear()?;
    Ok(())
}

pub fn leave(_terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> std::io::Result<()> {
    crossterm::execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()
}

fn restore() {
    let _ = crossterm::execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("chameleon: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(p) = &args.log
        && let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(p)
    {
        let _ = log::set_boxed_logger(Box::new(FileLog(std::sync::Mutex::new(f))));
        log::set_max_level(log::LevelFilter::Debug);
    }
    let cfg = match load_config(args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chameleon: {e}");
            return ExitCode::from(2);
        }
    };
    let client_tag = cfg
        .general
        .client_tag
        .clone()
        .unwrap_or_else(|| format!("cham-{}-{}", hostname(), std::process::id()));
    let runtime = match runtime::Runtime::new(cfg, client_tag, args.lua) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("chameleon: {e}");
            return ExitCode::from(2);
        }
    };

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
    }));
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout())) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("chameleon: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = enter(&mut terminal) {
        eprintln!("chameleon: {e}");
        return ExitCode::FAILURE;
    }
    let result = runtime.run(&mut terminal).await;
    restore();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chameleon: {e}");
            ExitCode::FAILURE
        }
    }
}
