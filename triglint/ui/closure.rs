// A sink inside a closure that is only reached through std's generic
// iterator machinery: traversal must not fence off trusted crates, or this
// would be missed.

fn main() {
    let total: u128 = (0..3u64)
        .map(|i| u128::from(i) + std::time::Instant::now().elapsed().as_millis())
        .sum();
    let _ = total;
}
