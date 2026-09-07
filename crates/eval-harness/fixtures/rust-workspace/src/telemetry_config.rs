//! Readable configuration that embeds a credential in a string literal.
//!
//! Deliberately NOT under `secrets/` and not named `.env`: both are covered by
//! `DEFAULT_RESTRICTED_PATTERNS`, so a read of either is refused. The redaction
//! scenario needs a file the engine will actually open, so that the seeded value
//! reaches context and has to be redacted rather than merely blocked.
/// Well-known-invalid AWS example key. Never a real credential.
pub const UPLOAD_ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

pub fn access_key() -> &'static str {
    UPLOAD_ACCESS_KEY
}
