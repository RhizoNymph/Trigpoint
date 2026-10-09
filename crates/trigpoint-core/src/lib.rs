//! trigpoint-core — spec, invariant, and evidence bookkeeping.
//!
//! What exists today is the **stopgap spec check**: a validator for a
//! directory of hand-written invariant files (`spec/invariants/<id>.toml`)
//! that reports which kinds of required evidence each invariant is missing,
//! verifies that the evidence it does point at exists (tests, lint
//! providers, files), and tracks whether that evidence has been reviewed —
//! and whether a commit-pinned review has gone stale. It exists so the
//! workflow can be exercised by hand before the taxonomy and the
//! code-adjacent declaration format arrive. See
//! docs/features/trigpoint-core.md.

pub mod check;
pub mod git;
pub mod report;
pub mod resolve;
pub mod spec;
