//! A single line text box.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub value: String,
    /// Cursor position in characters.
    pub cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        TextInput { value, cursor }
    }

    pub fn set(&mut self, value: impl Into<String>) {
        *self = TextInput::new(value);
    }

    fn byte_pos(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(i, _)| i)
    }

    pub fn insert_str(&mut self, s: &str) {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        let at = self.byte_pos(self.cursor);
        self.value.insert_str(at, &clean);
        self.cursor += clean.chars().count();
    }

    /// Handles editing keys. Returns true when the key was used.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let len = self.value.chars().count();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => self.set(""),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = len,
            KeyCode::Char(c) if !ctrl => self.insert_str(&c.to_string()),
            KeyCode::Backspace if self.cursor > 0 => {
                let at = self.byte_pos(self.cursor - 1);
                self.value.remove(at);
                self.cursor -= 1;
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_pos(self.cursor);
                self.value.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }

    /// The part of the text that fits in `width` columns, and where the
    /// cursor is inside it. Long text scrolls so the cursor stays visible.
    pub fn visible(&self, width: usize) -> (String, usize) {
        let chars: Vec<char> = self.value.chars().collect();
        if width == 0 {
            return (String::new(), 0);
        }
        let start = if self.cursor >= width {
            self.cursor + 1 - width
        } else {
            0
        };
        let shown: String = chars.iter().skip(start).take(width).collect();
        (shown, self.cursor - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    #[test]
    fn editing() {
        let mut t = TextInput::new("héllo");
        t.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(t.value, "héll");
        t.handle_key(KeyEvent::from(KeyCode::Home));
        t.handle_key(KeyEvent::from(KeyCode::Right));
        t.handle_key(KeyEvent::from(KeyCode::Right));
        t.handle_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(t.value, "hll");
        t.insert_str("e");
        assert_eq!(t.value, "hell");
        let (shown, cur) = TextInput::new("abcdefgh").visible(4);
        assert_eq!((shown.as_str(), cur), ("fgh", 3));
    }
}
