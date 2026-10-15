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
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => Config::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => Config::default(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Some(dir) = path.parent() {
        load_themes(&dir.join("themes"), &mut cfg);
    }
    // The theme last picked in the app wins over the config's.
    if let Some(saved) = state_path("theme").and_then(|p| std::fs::read_to_string(p).ok()) {
        let saved = saved.trim();
        cfg.theme.name = (!saved.is_empty()).then(|| saved.to_owned());
    }
    Ok(cfg)
}

/// Custom themes: `*.json` in Sessh's theme schema.
fn load_themes(dir: &std::path::Path, cfg: &mut Config) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    for p in paths {
        let parsed = std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| chameleon_app::themes::parse(&t));
        match parsed {
            Ok(themes) => cfg.custom_themes.extend(themes),
            Err(e) => cfg.problems.push(format!("{}: {e}", p.display())),
        }
    }
}

/// `$XDG_STATE_HOME/chameleon/<name>` or `~/.local/state/chameleon/<name>`.
pub fn state_path(name: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("chameleon").join(name))
}

/// Remembers the theme picked in the app (empty: the terminal's colours).
pub fn save_theme(name: Option<&str>) -> std::io::Result<()> {
    let Some(path) = state_path("theme") else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, name.unwrap_or_default())
}

/// Keeps what sessions spent for the next start, written whole and
/// renamed into place so a crash can't leave half a file.
pub fn save_usage(json: &str) -> std::io::Result<()> {
    let Some(path) = state_path("usage.json") else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

pub fn load_usage() -> Option<String> {
    std::fs::read_to_string(state_path("usage.json")?).ok()
}

/// This machine's offset from UTC in minutes (`date +%z`: `+0200` is 120),
/// that the usage overview splits days in.
pub fn utc_offset_minutes() -> i32 {
    let out = std::process::Command::new("date").arg("+%z").output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    parse_utc_offset(&text.unwrap_or_default()).unwrap_or(0)
}

fn parse_utc_offset(z: &str) -> Option<i32> {
    let (sign, digits) = match z.as_bytes().first()? {
        b'+' => (1, &z[1..]),
        b'-' => (-1, &z[1..]),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let h: i32 = digits[..2].parse().ok()?;
    let m: i32 = digits[2..].parse().ok()?;
    Some(sign * (h * 60 + m))
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

#[cfg(test)]
mod tests {
    use super::parse_utc_offset;

    #[test]
    fn utc_offsets_parse_from_date() {
        assert_eq!(parse_utc_offset("+0200"), Some(120));
        assert_eq!(parse_utc_offset("-0930"), Some(-570));
        assert_eq!(parse_utc_offset("+0000"), Some(0));
        assert_eq!(parse_utc_offset("CEST"), None);
    }
}
