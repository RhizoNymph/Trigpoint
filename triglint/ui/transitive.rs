// A sink reached through an intermediate local function: the diagnostic
// carries the full witness chain.

fn helper() {
    pause();
}

fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}

fn main() {
    helper();
}
