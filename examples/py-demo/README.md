# py-demo

End-to-end demo of the triglint determinism contract in Python — the analog
of `examples/sim-demo`, and the integration target for the Python linter
designed in `docs/features/python-linter.md` (not implemented yet).

A `ClockShim` protocol, a `SimClock` marked with `DeterministicShim`, a real
`SystemClock` (blessed to touch time by the `[[python.shims]]` grant in
`triglint.toml`), and a quarantined `pydemo.violate` module carrying the
documented `# triglint: allow(shim-nondeterminism)` escape hatch.

From this directory (or from anywhere with `-C examples/py-demo`):

```sh
trigp lint --python                 # clean: exits 0 with no diagnostics
```

## Layout

| module | role |
|---|---|
| `pydemo.shims` | `ClockShim` protocol, `SimClock` (deterministic), `run_tick` |
| `pydemo.prod` | `SystemClock`: the blessed `time` introduction point |
| `pydemo.sim.harness` | the declared sim root; module-level `main()` using `SimClock` only |
| `pydemo.violate` | deliberate violations; imported by nothing |

## What makes it clean

Sim mode walks the *import graph* from `pydemo.sim.harness`, so its closure
is `pydemo.shims` and nothing else of ours — no sink is named anywhere in it.
`pydemo.prod` and `pydemo.violate` are outside the closure and therefore
invisible to sim mode.

That import-closure rule is the one structural difference from the Rust demo.
There, `SystemClock` can sit in the same crate as `SimClock` because sim mode
is call-graph reachability and an uninstantiated impl is never reached. Here,
importing a module pulls its whole surface into sim scope, so the production
clock must live in its own module.

Prod mode scans every module instead, and allows a sink only inside a class
implementing a granting shim protocol: `time.time` is legal in
`SystemClock.now_millis` (`ClockShim` grants `time`) and would be illegal in
`SimClock`, which is marked `DeterministicShim` and so holds no grants.

## How to trigger violations

- **Sim violation.** Add `from pydemo.violate import coin_flip, wall_clock_tick`
  to `pydemo/sim/harness.py` and call them from `main()`. Sim mode then
  reports the `random` module fence and the `time.time` reference inside
  `SystemClock`, each with the witness chain
  `pydemo.sim.harness -> pydemo.violate -> …` down to the sink site. The allow
  comments in `pydemo.violate` do not help: they silence the introduction
  rule, not the simulation contract. This is the mirror of building sim-demo
  with `--features violate`.
- **Prod violation.** Delete an allow comment in `pydemo/violate.py`, or move
  `SystemClock.now_millis`'s body into a plain function: naming `time.time`
  outside a granting shim class is a `shim_nondeterminism` error.
- **Broken determinism claim.** Call `time.time()` from `SimClock.now_millis`.
  The `ClockShim` grant does not apply to a `DeterministicShim`-marked class,
  so both modes report it, and sim mode labels it a broken determinism claim.

## Note on external imports

`triglint.toml` here declares only `[python]`, `[python.sim]`,
`[[python.shims]]` and `[python.markers]`. The sim closure also imports
`typing` and `trigpoint_shims`, which are not under `source_roots`;
unresolvable imports are reported as holes (warnings, not errors) unless
listed in `[python.opaque] trusted_modules`. Add them there once the linter
exists if you want the run to be warning-free as well as error-free.
