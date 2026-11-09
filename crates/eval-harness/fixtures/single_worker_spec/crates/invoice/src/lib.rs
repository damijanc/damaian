//! Invoice lines.
//!
//! A line total is the quantity times the unit price in mills, converted to
//! cents by `pricing::to_cents`, so invoices always follow the pricing rule.

/// The total of one invoice line, in cents.
pub fn line_total(quantity: i64, unit_price_mills: i64) -> i64 {
    pricing::to_cents(quantity * unit_price_mills)
}

/// The total of several lines, in cents. Each line is converted on its own,
/// then the cents are added.
pub fn invoice_total(lines: &[(i64, i64)]) -> i64 {
    lines
        .iter()
        .map(|(quantity, unit_price_mills)| line_total(*quantity, *unit_price_mills))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{invoice_total, line_total};

    #[test]
    fn a_line_of_three_items_in_cents() {
        // 3 x 1.236 = 3.708
        assert_eq!(line_total(3, 1236), 370);
    }

    #[test]
    fn an_invoice_adds_its_lines() {
        assert_eq!(invoice_total(&[(1, 1000), (2, 2500)]), 600);
    }
}
