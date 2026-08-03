# sim-demo

End-to-end demo of the triglint determinism contract: a `ClockShim` trait,
a `SimClock` marked with `DeterministicShim`, a real `SystemClock` (blessed
to touch time by the `[[shims]]` grant in `triglint.toml`), and a hidden
sleep in a dependency function carrying the documented
`#[allow(shim_nondeterminism)]` escape hatch.

From this directory (or from anywhere with `-C examples/sim-demo`):

```sh
trigp lint                          # clean: exits 0 with no diagnostics
trigp lint -- --features violate    # exits 1 with two sim_nondeterminism
                                    # errors and full witness chains
```

The `violate` feature routes real wall time into the generic sim path and
calls the hidden dependency sleep — both are caught cross-crate, which is
why `trigp` runs the build with `DYLINT_RUSTFLAGS=-Zalways-encode-mir`.
Use `trigp lint --fresh` if you toggle MIR flags between runs; the flag
change alone does not invalidate cargo's cache.
