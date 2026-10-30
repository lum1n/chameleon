//! Opening links and copying text for the mouse, and reading images to
//! send with a prompt.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use chameleon_app::image::Image;

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

/// The image on this machine's clipboard: `wl-paste` or `xclip` on Linux,
/// AppleScript on macOS. Over SSH, the clipboard is the remote machine's,
/// which is seldom there to read.
pub fn image() -> Result<Image, String> {
    let bytes = if cfg!(target_os = "macos") {
        macos_image()?
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let types = output(&["wl-paste", "--list-types"])?;
        let ty = image_type(&types)?;
        output_bytes(&["wl-paste", "--no-newline", "--type", &ty])?
    } else if std::env::var_os("DISPLAY").is_some() {
        let types = output(&["xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"])?;
        let ty = image_type(&types)?;
        output_bytes(&["xclip", "-selection", "clipboard", "-t", &ty, "-o"])?
    } else {
        return Err(if std::env::var_os("SSH_CONNECTION").is_some() {
            "no clipboard to read over SSH; paste an image's path instead".into()
        } else {
            "no clipboard to read (no Wayland or X display)".into()
        });
    };
    Image::from_bytes("clipboard", &bytes)
}

/// The image files a paste names, read, when it names nothing else and
/// they are all there; `None` leaves the paste as text.
pub fn pasted_images(text: &str) -> Option<Vec<Result<Image, String>>> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let paths: Vec<PathBuf> = chameleon_app::image::pasted_paths(text)?
        .into_iter()
        .map(|p| match (p.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(p),
        })
        .collect();
    if !paths.iter().all(|p| p.is_file()) {
        return None;
    }
    Some(
        paths
            .iter()
            .map(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let bytes = std::fs::read(p).map_err(|e| format!("can't read {name}: {e}"))?;
                Image::from_bytes(name, &bytes)
            })
            .collect(),
    )
}

/// The first image type a clipboard offers that every harness takes.
fn image_type(types: &str) -> Result<String, String> {
    const TAKEN: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
    TAKEN
        .iter()
        .find(|t| types.lines().any(|l| l.trim() == **t))
        .map(|t| (*t).to_owned())
        .ok_or_else(|| "no image on the clipboard".to_owned())
}

/// The clipboard as PNG, which AppleScript prints as `«data PNGf89504E…»`.
fn macos_image() -> Result<Vec<u8>, String> {
    let out = output(&["osascript", "-e", "the clipboard as «class PNGf»"])
        .map_err(|_| "no image on the clipboard".to_owned())?;
    let hex = out
        .trim()
        .strip_prefix("«data PNGf")
        .and_then(|h| h.strip_suffix('»'))
        .ok_or("no image on the clipboard")?;
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("can't read the clipboard: {e}"))
}

fn output(argv: &[&str]) -> Result<String, String> {
    output_bytes(argv).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// What `argv` prints, when it runs and succeeds.
fn output_bytes(argv: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("can't run {}: {e}", argv[0]))?;
    if !out.status.success() {
        return Err("no image on the clipboard".into());
    }
    Ok(out.stdout)
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
