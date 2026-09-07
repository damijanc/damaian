/// Applies a discount code to a basket total. The conceptual-feature scenario
/// searches for "discount" without naming this file.
pub fn apply_discount(total_cents: u64, code: &str) -> u64 {
    match code {
        "HALF" => total_cents / 2,
        "TENOFF" => total_cents.saturating_sub(1000),
        _ => total_cents,
    }
}
