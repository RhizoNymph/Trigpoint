// edition:2021
// (the ui harness compiles fixtures as 2015 unless told otherwise)
//
// An async fn's body only runs inside `Future::poll`, and executors reach
// that through `dyn Future` vtables. Without vtable collection the sink in
// `work` is invisible to the analysis.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Waker};

async fn work() -> u64 {
    std::time::Instant::now().elapsed().as_secs()
}

fn main() {
    let mut fut = std::pin::pin!(work());
    let dynamic: Pin<&mut dyn Future<Output = u64>> = fut.as_mut();
    let mut cx = Context::from_waker(Waker::noop());
    let _ = dynamic.poll(&mut cx);
}
