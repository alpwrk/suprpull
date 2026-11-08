//! Single-line text input with a char-indexed cursor.

#[derive(Debug, Default, Clone)]
pub struct Input {
    text: String,
    /// Cursor position in chars (0..=char_count).
    cursor: usize,
}

impl Input {
    #[cfg(test)]
    pub fn value(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_index(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    pub fn insert(&mut self, c: char) {
        let i = self.byte_index(self.cursor);
        self.text.insert(i, c);
        self.cursor += 1;
    }

    /// Insert a string, dropping control characters (newlines from pastes etc.).
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            self.insert(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let i = self.byte_index(self.cursor);
        self.text.remove(i);
    }

    pub fn delete(&mut self) {
        if self.cursor < self.char_count() {
            let i = self.byte_index(self.cursor);
            self.text.remove(i);
        }
    }

    /// Delete the word before the cursor (Ctrl-W).
    pub fn delete_word(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let (a, b) = (self.byte_index(start), self.byte_index(self.cursor));
        self.text.replace_range(a..b, "");
        self.cursor = start;
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_count());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.char_count();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Take the trimmed value and clear the field.
    pub fn take(&mut self) -> String {
        let v = self.text.trim().to_string();
        self.clear();
        v
    }

    /// The part of the text visible in a field `width` chars wide, plus the
    /// cursor column relative to the field start. Scrolls so the cursor stays visible.
    pub fn visible(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }
        let scroll = self.cursor.saturating_sub(width - 1);
        let s: String = self.text.chars().skip(scroll).take(width).collect();
        (s, self.cursor - scroll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Input {
        let mut i = Input::default();
        i.insert_str(s);
        i
    }

    #[test]
    fn insert_and_move() {
        let mut i = typed("ac");
        i.left();
        i.insert('b');
        assert_eq!(i.value(), "abc");
        assert_eq!(i.cursor(), 2);
        i.home();
        i.insert('>');
        assert_eq!(i.value(), ">abc");
        i.end();
        i.right();
        assert_eq!(i.cursor(), 4);
    }

    #[test]
    fn backspace_delete_unicode() {
        let mut i = typed("äöü");
        i.backspace();
        assert_eq!(i.value(), "äö");
        i.home();
        i.delete();
        assert_eq!(i.value(), "ö");
        i.home();
        i.backspace();
        assert_eq!(i.value(), "ö");
    }

    #[test]
    fn delete_word_and_paste_strips_newlines() {
        let mut i = typed("foo bar  ");
        i.delete_word();
        assert_eq!(i.value(), "foo ");
        let mut p = Input::default();
        p.insert_str("https://x\r\n");
        assert_eq!(p.value(), "https://x");
    }

    #[test]
    fn visible_scrolls_with_cursor() {
        let i = typed("abcdefghij");
        let (s, c) = i.visible(5);
        assert_eq!(s, "ghij");
        assert_eq!(c, 4);
        assert_eq!(i.visible(0), (String::new(), 0));
        let mut j = i.clone();
        j.home();
        assert_eq!(j.visible(5), ("abcde".to_string(), 0));
    }

    #[test]
    fn take_trims_and_clears() {
        let mut i = typed("  url ");
        assert_eq!(i.take(), "url");
        assert!(i.is_empty());
        assert_eq!(i.cursor(), 0);
    }
}
