/// Splits a `key=value` configuration line into its two halves.
pub fn parse_line(line: &str) -> Option<(&str, &str)> {
    line.split_once('=')
        .map(|(key, value)| (key.trim(), value.trim()))
}
