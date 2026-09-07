/// Uploads one payload. Has no retry, which is what the patch scenarios add.
pub fn upload(payload: &str) -> Result<(), String> {
    if payload.is_empty() {
        return Err("payload was empty".to_string());
    }
    Ok(())
}
