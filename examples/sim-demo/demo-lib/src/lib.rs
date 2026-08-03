//! Toy shim library for the triglint end-to-end demo.

use trigpoint_shims::DeterministicShim;

pub trait ClockShim {
    fn now_millis(&mut self) -> u64;
}

/// Simulation clock: virtual time, fully deterministic.
pub struct SimClock {
    now: u64,
}

impl SimClock {
    pub fn new(start: u64) -> Self {
        Self { now: start }
    }
}

impl DeterministicShim for SimClock {}

impl ClockShim for SimClock {
    fn now_millis(&mut self) -> u64 {
        self.now += 1;
        self.now
    }
}

/// Production clock: reads real wall time. Fine in production, a violation
/// if it leaks into a simulation build.
pub struct SystemClock;

impl ClockShim for SystemClock {
    fn now_millis(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default()
    }
}

pub fn run_tick<C: ClockShim>(clock: &mut C) -> u64 {
    clock.now_millis()
}

/// Nondeterminism hidden inside a non-generic dependency function: only
/// visible to sim mode through cross-crate MIR (-Zalways-encode-mir).
///
/// Prod mode would flag the sleep here (it is outside any shim impl); the
/// allow demonstrates the escape hatch — this function exists precisely to
/// be a hidden sink for the sim-mode demo. `unknown_lints` is allowed
/// because plain `cargo build` doesn't register triglint's lints.
#[allow(unknown_lints)]
#[allow(shim_nondeterminism)]
pub fn wait_a_bit() {
    std::thread::sleep(std::time::Duration::from_millis(10));
}
