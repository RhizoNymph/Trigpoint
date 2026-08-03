// Dyn dispatch is out of scope for resolution: the analysis reports the
// hole instead of assuming the callee is safe.

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
