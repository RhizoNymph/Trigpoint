// Unaudited FFI is unauditable nondeterminism: denied by default.

unsafe extern "C" {
    fn getpid() -> i32;
}

fn main() {
    let _ = unsafe { getpid() };
}
