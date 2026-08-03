// The intended pattern: nondeterminism behind a shim trait, with the sim
// impl marked deterministic and genuinely sink-free. No diagnostics.

mod shims {
    pub trait DeterministicShim {}
}

trait ClockShim {
    fn now_millis(&mut self) -> u64;
}

struct SimClock {
    t: u64,
}

impl shims::DeterministicShim for SimClock {}

impl ClockShim for SimClock {
    fn now_millis(&mut self) -> u64 {
        self.t += 1;
        self.t
    }
}

fn step<C: ClockShim>(clock: &mut C) -> u64 {
    clock.now_millis()
}

fn main() {
    let mut clock = SimClock { t: 0 };
    let _ = step(&mut clock);
}
