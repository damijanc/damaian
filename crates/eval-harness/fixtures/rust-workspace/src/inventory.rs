/// Returns whether the requested quantity can be fulfilled from stock on hand.
pub fn can_fulfil(on_hand: u64, requested: u64) -> bool {
    on_hand >= requested
}
