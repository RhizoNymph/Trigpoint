# trigpoint-shims

Marker base classes consumed by trigpoint's Python determinism linter — the
PyPI sibling of the `trigpoint-shims` Rust crate.

The package exports exactly one name:

```python
from trigpoint_shims import DeterministicShim


class SimClock(ClockShim, DeterministicShim):
    """Virtual time: no nondeterminism sink is reachable through any method."""

    def now_millis(self) -> int: ...
```

`DeterministicShim` is a pure marker: no methods, no attributes, no
`__init__`, no runtime dependencies. Analyzed codebases depend on it solely
to *declare* intent, never for behavior — importing it costs a project
nothing, and subclassing it changes nothing at runtime.

What the marker claims, and what the linter checks: no nondeterminism sink —
time, random, I/O, threads, subprocesses, environment — is reachable through
any method of the marked type. The linter verifies this over the import
closure of the declared simulation roots and attributes any sink it finds
inside a marked type as a *broken determinism claim* rather than an ordinary
violation.

Declare the marker in `triglint.toml` so the linter knows to look for it:

```toml
[python.markers]
deterministic = ["trigpoint_shims.DeterministicShim"]
```

See `docs/features/python-linter.md` in the trigpoint repository for the
analysis design, and `examples/py-demo/` for a worked example.

Licensed under Apache-2.0.
