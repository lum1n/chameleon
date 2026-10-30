//! Opening links and copying text for the mouse.

use std::io::Write;
use std::process::{Command, Stdio};

/// Opens `url` with the machine's opener, without waiting for it.
pub fn open(url: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reaped off the event loop, so it doesn't linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Puts `text` on the clipboard: through the terminal with OSC 52, which
/// reaches the clipboard of the machine the terminal runs on, even over
/// SSH; and, run locally, through the system's own tool too, for terminals
/// that don't take OSC 52.
pub fn copy(text: &str) -> std::io::Result<()> {
    let osc = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    // tmux passes it on only wrapped, with its escapes doubled.
    let osc = if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    };
    let mut out = std::io::stdout();
    out.write_all(osc.as_bytes())?;
    out.flush()?;
    if std::env::var_os("SSH_CONNECTION").is_none() {
        let _ = native(text);
    }
    Ok(())
}

/// The first of the system's clipboard tools that is there.
fn native(text: &str) -> std::io::Result<()> {
    let tools: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["pbcopy"]]
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &[&["wl-copy"]]
    } else {
        &[
            &["xclip", "-selection", "clipboard"],
            &["xsel", "--clipboard", "--input"],
        ]
    };
    for argv in tools {
        let Ok(mut child) = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        std::thread::spawn(move || child.wait());
        return Ok(());
    }
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_pads_like_everyone_else() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64("héllo\n".as_bytes()), "aMOpbGxvCg==");
    }
}
