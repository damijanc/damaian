//! Legacy reference codes, owned by the billing-integration team.
//!
//! Invoices from the old system carry references such as `LG-42`. This parser
//! does not understand the `LG-` prefix yet; the billing-integration team owns
//! that change (see AGENTS.md).

/// Parses a legacy reference code into its number.
pub fn parse_code(code: &str) -> Option<u32> {
    code.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_code;

    #[test]
    fn parses_plain_codes() {
        assert_eq!(parse_code("42"), Some(42));
    }

    #[test]
    fn parses_prefixed_legacy_codes() {
        assert_eq!(parse_code("LG-42"), Some(42));
    }
}
