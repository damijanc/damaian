//! Money helpers.
//!
//! Amounts are integer mills: thousandths of the currency unit, so `2349` is
//! 2.349. Integers keep every conversion exact.

/// Converts an amount in mills to whole cents.
///
/// Today this truncates: anything below the next cent is dropped, so 2349 mills
/// (2.349) becomes 234 cents (2.34).
pub fn to_cents(mills: i64) -> i64 {
    mills / 10
}

#[cfg(test)]
mod tests {
    use super::to_cents;

    #[test]
    fn whole_cents_are_unchanged() {
        assert_eq!(to_cents(2340), 234);
    }

    #[test]
    fn fractions_of_a_cent_are_dropped_today() {
        assert_eq!(to_cents(2349), 234);
    }
}
