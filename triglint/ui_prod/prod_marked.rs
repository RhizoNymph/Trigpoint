// A sim impl (marked deterministic) touching time: the ClockShim grant does
// NOT apply, because marked impls receive no grants. Caught by prod mode
// even though nothing reaches it from the sim root.

mod shims {
    pub trait DeterministicShim {}
}

trait ClockShim {
    fn now_millis(&mut self) -> u64;
}

struct SimClock;

impl shims::DeterministicShim for SimClock {}

impl ClockShim for SimClock {
    fn now_millis(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default()
    }
}

fn main() {}
