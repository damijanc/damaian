/// Writes a line to the process log with a severity prefix.
pub fn log_line(severity: &str, message: &str) -> String {
    format!("[{severity}] {message}")
}
