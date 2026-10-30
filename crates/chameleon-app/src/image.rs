//! Images sent with a prompt: told apart by their bytes, and found in
//! pasted text, which is how a terminal hands over a file dropped on it.
//!
//! The image goes inline in the prompt (base64, see
//! [`acc_core::command::Attachment`]), so it reaches a harness on another
//! host over the same pipe as the text; nothing is copied there first.

use acc_core::command::Attachment;

/// An image waiting in the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// The file name, or `clipboard`.
    pub name: String,
    pub attachment: Attachment,
}

impl Image {
    /// The image in `bytes`, when it is one every harness takes and small
    /// enough to send.
    pub fn from_bytes(name: impl Into<String>, bytes: &[u8]) -> Result<Self, String> {
        let name = name.into();
        let Some(media_type) = sniff(bytes) else {
            return Err(format!("{name} isn't a PNG, JPEG, GIF or WebP image"));
        };
        if bytes.len() > Attachment::MAX_BYTES {
            return Err(format!(
                "{name} is {}, the limit is {}",
                size(bytes.len()),
                size(Attachment::MAX_BYTES)
            ));
        }
        Ok(Self {
            name,
            attachment: Attachment::from_bytes(media_type, bytes),
        })
    }

    /// `shot.png 240 KB`, for the composer.
    pub fn label(&self) -> String {
        format!("{} {}", self.name, size(self.attachment.raw_len()))
    }
}

/// The media type of an image, from its first bytes.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The image files in pasted text, when it is nothing but paths to them:
/// one per line, or separated by spaces with the shell quoting terminals
/// use for dropped files (`'/a b.png'`, `/a\ b.png`), or `file://` URLs.
/// Paths are absolute or start with `~`, so a prompt that merely mentions
/// `logo.png` stays text.
pub fn pasted_paths(text: &str) -> Option<Vec<String>> {
    let paths = split_words(text.trim())?;
    let image = |p: &str| {
        let ext = p.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        (p.starts_with('/') || p.starts_with("~/"))
            && matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp"))
    };
    let paths: Vec<String> = paths
        .into_iter()
        .map(|w| match w.strip_prefix("file://") {
            Some(rest) => percent_decode(rest),
            None => w,
        })
        .collect();
    (!paths.is_empty() && paths.iter().all(|p| image(p))).then_some(paths)
}

/// Splits like a shell does, quotes and backslashes; `None` on an open quote.
fn split_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => word.push(chars.next()?),
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.push(chars.next()?);
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `240 KB`, `1.2 MB`.
fn size(bytes: usize) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / f64::from(1 << 20))
    } else {
        format!("{} KB", bytes.div_ceil(1 << 10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_images_from_their_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"<svg"), None);
        let err = Image::from_bytes("notes.txt", b"hello").unwrap_err();
        assert!(err.contains("notes.txt isn't"), "{err}");
        let img = Image::from_bytes("a.png", b"\x89PNG\r\n\x1a\nxx").unwrap();
        assert_eq!(img.attachment.media_type, "image/png");
        assert_eq!(img.label(), "a.png 1 KB");
    }

    #[test]
    fn finds_dropped_files_in_a_paste() {
        let p = |t: &str| pasted_paths(t);
        assert_eq!(p("/tmp/a.png"), Some(vec!["/tmp/a.png".into()]));
        assert_eq!(
            p("  '/tmp/my shot.PNG' \n"),
            Some(vec!["/tmp/my shot.PNG".into()])
        );
        assert_eq!(
            p("/tmp/a\\ b.jpg ~/c.webp"),
            Some(vec!["/tmp/a b.jpg".into(), "~/c.webp".into()])
        );
        assert_eq!(p("/a.png\n/b.gif"), Some(vec!["/a.png".into(), "/b.gif".into()]));
        assert_eq!(p("file:///tmp/a%20b.png"), Some(vec!["/tmp/a b.png".into()]));
        assert_eq!(p("look at /tmp/a.png"), None);
        assert_eq!(p("logo.png"), None);
        assert_eq!(p("/tmp/a.png /etc/passwd"), None);
        assert_eq!(p("'/tmp/a.png"), None);
        assert_eq!(p(""), None);
    }
}
