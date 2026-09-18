// A sink behind dyn dispatch. The call itself is virtual, but the vtable
// can only exist because it was constructed at a reachable coercion site,
// so the impl's methods are collected there and the sink is found.

trait Api {
    fn go(&self) -> u64;
}

struct Wall;

impl Api for Wall {
    fn go(&self) -> u64 {
        std::time::Instant::now().elapsed().as_secs()
    }
}

fn run(api: &dyn Api) -> u64 {
    api.go()
}

fn main() {
    let wall = Wall;
    let _ = run(&wall);
}
