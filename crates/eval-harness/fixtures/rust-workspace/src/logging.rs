/// Writes a line to the process log with a severity prefix, such as a promotion being applied.
pub fn log_line(severity: &str, message: &str) -> String {
    format!("[{severity}] {message}")
}
