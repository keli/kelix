pub fn byte_index_at_char(s: &str, char_pos: usize) -> usize {
    if char_pos == 0 {
        return 0;
    }
    s.char_indices()
        .nth(char_pos)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len())
}

pub fn insert_char_at_cursor(input_buf: &mut String, cursor_char_pos: &mut usize, c: char) {
    let byte_pos = byte_index_at_char(input_buf, *cursor_char_pos);
    input_buf.insert(byte_pos, c);
    *cursor_char_pos += 1;
}

pub fn backspace_char_at_cursor(input_buf: &mut String, cursor_char_pos: &mut usize) {
    if *cursor_char_pos == 0 {
        return;
    }
    let delete_from = byte_index_at_char(input_buf, *cursor_char_pos - 1);
    let delete_to = byte_index_at_char(input_buf, *cursor_char_pos);
    input_buf.drain(delete_from..delete_to);
    *cursor_char_pos -= 1;
}

pub fn delete_char_at_cursor(input_buf: &mut String, cursor_char_pos: usize) {
    let len = input_buf.chars().count();
    if cursor_char_pos >= len {
        return;
    }
    let delete_from = byte_index_at_char(input_buf, cursor_char_pos);
    let delete_to = byte_index_at_char(input_buf, cursor_char_pos + 1);
    input_buf.drain(delete_from..delete_to);
}
