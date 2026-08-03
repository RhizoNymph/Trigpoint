// A shim impl that claims determinism via the marker trait but reaches a
// sink: the diagnostic calls out the broken claim.

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

fn step<C: ClockShim>(clock: &mut C) -> u64 {
    clock.now_millis()
}

fn main() {
    let mut clock = SimClock;
    let _ = step(&mut clock);
}
