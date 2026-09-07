/// Truncates a string to at most `limit` characters, without splitting a char.
pub fn truncate(input: &str, limit: usize) -> &str {
    match input.char_indices().nth(limit) {
        Some((index, _)) => &input[..index],
        None => input,
    }
}
