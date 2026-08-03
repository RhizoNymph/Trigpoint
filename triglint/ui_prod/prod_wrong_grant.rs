// A shim impl touching a capability its trait does not grant: ClockShim
// grants time, not thread.

trait ClockShim {
    fn now_millis(&mut self) -> u64;
}

struct SleepyClock;

impl ClockShim for SleepyClock {
    fn now_millis(&mut self) -> u64 {
        std::thread::sleep(std::time::Duration::from_millis(1));
        0
    }
}

fn main() {}
