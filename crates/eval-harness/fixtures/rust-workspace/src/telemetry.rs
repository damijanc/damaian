/// Counts how many times an event name has been seen this session.
pub fn increment(counter: &mut u64) -> u64 {
    *counter += 1;
    *counter
}
