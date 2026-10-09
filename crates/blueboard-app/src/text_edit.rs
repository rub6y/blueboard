//! Char-index-safe editing operations for the text tool's in-progress string
//! (`App::text_input_text`/`text_input_cursor`) — a plain `String` plus a byte offset
//! would risk splitting a multi-byte UTF-8 character on backspace/insert; operating in
//! terms of `chars().collect::<Vec<_>>()` instead sidesteps that entirely. Kept as free
//! functions over `&mut String`/`&mut usize` rather than a wrapper type since `App`
//! already owns both fields directly and splits borrows on them the same way elsewhere.

/// Inserts `text` at the character index `*cursor`, then advances `*cursor` past it.
pub fn insert(content: &mut String, cursor: &mut usize, text: &str) {
    let mut chars: Vec<char> = content.chars().collect();
    let at = (*cursor).min(chars.len());
    for (offset, c) in text.chars().enumerate() {
        chars.insert(at + offset, c);
    }
    *cursor = at + text.chars().count();
    *content = chars.into_iter().collect();
}

/// Removes the character just before `*cursor` (backspace) — a no-op at the start.
pub fn backspace(content: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let mut chars: Vec<char> = content.chars().collect();
    chars.remove(*cursor - 1);
    *cursor -= 1;
    *content = chars.into_iter().collect();
}

/// Removes the character just after `*cursor` (delete) — a no-op at the end.
pub fn delete_forward(content: &mut String, cursor: usize) {
    let mut chars: Vec<char> = content.chars().collect();
    if cursor < chars.len() {
        chars.remove(cursor);
        *content = chars.into_iter().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_at_cursor_advances_it_past_the_inserted_text() {
        let mut content = "ac".to_string();
        let mut cursor = 1;
        insert(&mut content, &mut cursor, "b");
        assert_eq!(content, "abc");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn backspace_removes_before_cursor_and_is_a_no_op_at_start() {
        let mut content = "abc".to_string();
        let mut cursor = 2;
        backspace(&mut content, &mut cursor);
        assert_eq!(content, "ac");
        assert_eq!(cursor, 1);

        let mut cursor = 0;
        backspace(&mut content, &mut cursor);
        assert_eq!(content, "ac");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn delete_forward_removes_after_cursor_and_is_a_no_op_at_end() {
        let mut content = "abc".to_string();
        delete_forward(&mut content, 1);
        assert_eq!(content, "ac");

        delete_forward(&mut content, 2);
        assert_eq!(content, "ac");
    }

    #[test]
    fn multi_byte_characters_are_not_split() {
        let mut content = "héllo".to_string();
        let mut cursor = 2; // between 'é' and 'l'
        backspace(&mut content, &mut cursor);
        assert_eq!(content, "hllo");
        assert_eq!(cursor, 1);
    }
}
