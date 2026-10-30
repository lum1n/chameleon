//! The prompt editor: a small multi-line text buffer with a cursor, and
//! the images that go with it.

use crate::image::Image;
use crate::keymap::{Key, KeyCode};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composer {
    text: String,
    /// Cursor as a char index into `text`.
    cursor: usize,
    history: Vec<String>,
    history_pos: Option<usize>,
    /// Images to send with the text, in the order they came.
    images: Vec<Image>,
}

impl Composer {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// No text and no images.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.images.is_empty()
    }

    pub fn images(&self) -> &[Image] {
        &self.images
    }

    pub fn add_image(&mut self, image: Image) {
        self.images.push(image);
    }

    /// Takes the images out for sending.
    pub fn take_images(&mut self) -> Vec<Image> {
        std::mem::take(&mut self.images)
    }

    /// Puts back a prompt that couldn't be sent.
    pub fn restore(&mut self, text: &str, images: Vec<Image>) {
        self.set(text);
        self.images = images;
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = self.text.chars().count();
    }

    /// Takes the text out for sending and remembers it.
    pub fn take(&mut self) -> String {
        let t = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.history_pos = None;
        if !t.trim().is_empty() && self.history.last() != Some(&t) {
            self.history.push(t.clone());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
        t
    }

    pub fn insert(&mut self, s: &str) {
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, s);
        self.cursor += s.chars().count();
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    /// (line, column) of the cursor, in chars.
    pub fn cursor_pos(&self) -> (usize, usize) {
        let before: String = self.text.chars().take(self.cursor).collect();
        let line = before.matches('\n').count();
        let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0);
        (line, col)
    }

    /// Handles an editing key. Returns false if the key wasn't for the editor.
    pub fn key(&mut self, key: &Key) -> bool {
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Char(c) if !key.ctrl && !key.alt => {
                self.insert(&c.to_string());
                self.history_pos = None;
            }
            KeyCode::Enter if key.alt || key.shift => self.insert("\n"),
            KeyCode::Backspace => {
                if key.alt || key.ctrl {
                    self.delete_word();
                } else if self.cursor == 0 {
                    // The images sit before the text; the last one goes.
                    return self.images.pop().is_some();
                } else if self.cursor > 0 {
                    let at = self.byte_at(self.cursor - 1);
                    self.text.remove(at);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if self.cursor < len {
                    let at = self.byte_at(self.cursor);
                    self.text.remove(at);
                }
            }
            KeyCode::Left if key.ctrl || key.alt => self.cursor = self.word_left(),
            KeyCode::Right if key.ctrl || key.alt => self.cursor = self.word_right(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => {
                let (_, col) = self.cursor_pos();
                self.cursor -= col;
            }
            KeyCode::End => {
                let rest: String = self.text.chars().skip(self.cursor).collect();
                let to_eol = rest
                    .find('\n')
                    .map(|i| rest[..i].chars().count())
                    .unwrap_or(rest.chars().count());
                self.cursor += to_eol;
            }
            KeyCode::Up => {
                let (line, col) = self.cursor_pos();
                if line == 0 {
                    return self.history_up();
                }
                self.move_to_line(line - 1, col);
            }
            KeyCode::Down => {
                let (line, col) = self.cursor_pos();
                let lines = self.text.matches('\n').count();
                if line >= lines {
                    return self.history_down();
                }
                self.move_to_line(line + 1, col);
            }
            KeyCode::Char('a') if key.ctrl => self.cursor = 0,
            KeyCode::Char('e') if key.ctrl => self.cursor = len,
            KeyCode::Char('w') if key.ctrl => self.delete_word(),
            KeyCode::Char('k') if key.ctrl => {
                let at = self.byte_at(self.cursor);
                self.text.truncate(at);
            }
            KeyCode::Char('u') if key.ctrl => {
                let at = self.byte_at(self.cursor);
                self.text.drain(..at);
                self.cursor = 0;
            }
            _ => return false,
        }
        true
    }

    fn move_to_line(&mut self, line: usize, col: usize) {
        let mut idx = 0;
        for (i, l) in self.text.split('\n').enumerate() {
            let n = l.chars().count();
            if i == line {
                self.cursor = idx + col.min(n);
                return;
            }
            idx += n + 1;
        }
    }

    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        i
    }

    fn delete_word(&mut self) {
        let from = self.word_left();
        let a = self.byte_at(from);
        let b = self.byte_at(self.cursor);
        self.text.drain(a..b);
        self.cursor = from;
    }

    fn history_up(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let pos = match self.history_pos {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        let t = self.history[pos].clone();
        self.set(&t);
        true
    }

    fn history_down(&mut self) -> bool {
        let Some(pos) = self.history_pos else { return false };
        if pos + 1 >= self.history.len() {
            self.history_pos = None;
            self.set("");
        } else {
            self.history_pos = Some(pos + 1);
            let t = self.history[pos + 1].clone();
            self.set(&t);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_and_cursor() {
        let mut c = Composer::default();
        for ch in "héllo wörld".chars() {
            c.key(&Key::char(ch));
        }
        assert_eq!(c.text(), "héllo wörld");
        c.key(&Key::ctrl(KeyCode::Char('w')));
        assert_eq!(c.text(), "héllo ");
        c.key(&Key::alt(KeyCode::Enter));
        c.key(&Key::char('x'));
        assert_eq!(c.cursor_pos(), (1, 1));
        c.key(&Key::new(KeyCode::Up));
        assert_eq!(c.cursor_pos(), (0, 1));
        let t = c.take();
        assert_eq!(t, "héllo \nx");
        assert!(c.is_empty());
        c.key(&Key::new(KeyCode::Up));
        assert_eq!(c.text(), t);
    }

    #[test]
    fn backspace_at_the_start_drops_the_last_image() {
        let mut c = Composer::default();
        let png = |n: &str| Image::from_bytes(n, b"\x89PNG\r\n\x1a\n").unwrap();
        c.add_image(png("a.png"));
        c.add_image(png("b.png"));
        assert!(!c.is_empty(), "images alone are a prompt");
        c.insert("x");
        c.key(&Key::new(KeyCode::Backspace));
        assert_eq!(c.images().len(), 2, "the text goes first");
        c.key(&Key::new(KeyCode::Backspace));
        assert_eq!(c.images()[0].name, "a.png");
        assert!(c.key(&Key::new(KeyCode::Backspace)));
        assert!(c.is_empty());
        assert!(!c.key(&Key::new(KeyCode::Backspace)), "nothing left to drop");
    }
}
