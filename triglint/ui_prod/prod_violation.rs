// Nondeterminism introduced outside any shim impl. Note none of these are
// called from the sim root: prod mode flags introduction points per-crate,
// independent of reachability.

trait ClockShim {
    fn now_millis(&mut self) -> u64;
}

fn sneaky_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn sneaky_sleep_in_closure() {
    let f = || std::thread::sleep(std::time::Duration::from_millis(1));
    f();
}

#[allow(shim_nondeterminism)]
fn intentionally_allowed() {
    // The escape hatch: item-level allow is respected (node-level emission).
    std::thread::sleep(std::time::Duration::from_millis(1));
}

fn main() {}
