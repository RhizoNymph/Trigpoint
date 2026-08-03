//! Marker traits consumed by `triglint`.
//!
//! This crate must stay dependency-free and stable: analyzed projects depend
//! on it solely to *declare* intent, never for behavior.

/// Declares that a shim implementation is deterministic.
///
/// Implement this on the simulation variant of a shim (e.g. `SimClock` for a
/// `ClockShim` trait). The contract: no nondeterminism sink — time, random,
/// I/O, threads, FFI — is reachable through any method of the marked type.
/// `triglint` verifies the contract via whole-program MIR reachability from
/// the simulation roots; a sink reached through a marked impl is reported as
/// a broken determinism claim.
pub trait DeterministicShim {}
