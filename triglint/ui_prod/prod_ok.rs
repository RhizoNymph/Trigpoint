// The sanctioned prod pattern: the real clock touches time *inside* its
// ClockShim impl (granted), and prod code consumes time only through the
// shim trait. No diagnostics — note main never reaches the real clock, so
// sim mode is silent too.

trait ClockShim {
    fn now_millis(&mut self) -> u64;
}

struct SystemClock;

impl ClockShim for SystemClock {
    fn now_millis(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default()
    }
}

struct FixedClock(u64);

impl ClockShim for FixedClock {
    fn now_millis(&mut self) -> u64 {
        self.0
    }
}

fn tick<C: ClockShim>(clock: &mut C) -> u64 {
    clock.now_millis()
}

fn main() {
    let mut clock = FixedClock(7);
    let _ = tick(&mut clock);
}
