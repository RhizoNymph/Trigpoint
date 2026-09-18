// Function pointers and trait objects can be baked into constants and
// statics, where no cast in any body reifies them. Following the constants'
// provenance recovers the targets.

trait Handler {
    fn handle(&self);
}

struct Sleeper;

impl Handler for Sleeper {
    fn handle(&self) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

static STAMPS: [fn() -> u64; 1] = [stamp];

const HANDLER: &dyn Handler = &Sleeper;

fn main() {
    let _ = STAMPS[0]();
    HANDLER.handle();
}
