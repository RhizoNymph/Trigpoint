// Dyn dispatch over a clean impl: collecting the vtable's methods at the
// coercion site must not manufacture a diagnostic. No output.

trait Api {
    fn go(&self);
}

struct A;

impl Api for A {
    fn go(&self) {}
}

fn main() {
    let a = A;
    let api: &dyn Api = &a;
    api.go();
}
