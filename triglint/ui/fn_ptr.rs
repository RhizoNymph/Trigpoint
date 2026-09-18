// Function pointers: both a reified `fn` item and a coerced non-capturing
// closure. Each target is collected where the pointer is created, so the
// indirect call sites do not hide the sinks behind them.

fn tick() -> u64 {
    std::time::Instant::now().elapsed().as_secs()
}

fn main() {
    let f: fn() -> u64 = tick;
    let _ = f();

    let g: fn() = || std::thread::sleep(std::time::Duration::from_millis(1));
    g();
}
