//! Simulation harness: the sim root declared in triglint.toml.
//!
//! Default build is deterministic and lints clean. `--features violate`
//! routes real time and a hidden dependency sleep into the simulation,
//! which triglint must reject.

fn main() {
    let mut clock = demo_lib::SimClock::new(0);
    let t = demo_lib::run_tick(&mut clock);
    println!("sim tick at virtual t={t}");

    #[cfg(feature = "violate")]
    violate();
}

#[cfg(feature = "violate")]
fn violate() {
    // Cross-crate transitive sink: sleep hidden inside a non-generic
    // dependency function.
    demo_lib::wait_a_bit();
    // Real clock instantiated into the generic sim path.
    let mut real = demo_lib::SystemClock;
    let _ = demo_lib::run_tick(&mut real);
}
