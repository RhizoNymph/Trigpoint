// The motivating indirect case: `format_args!` reifies `Display::fmt` into a
// function pointer inside core's argument machinery, so a sink in a user
// `Display` impl is only reachable through that pointer.

use std::fmt;

struct Uptime;

impl fmt::Display for Uptime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        write!(f, "{secs}")
    }
}

fn main() {
    let uptime = Uptime;
    println!("{uptime}");
}
