# python-linter — DST shim linting for Python codebases (planned, v0)

Status: **intent only — no implementation yet.** This doc records the scope
and open questions so the workstream can be picked up.

## Scope

Bring the trigpoint DST contract to Python codebases: all nondeterminism
(time, randomness, I/O, threads, subprocesses, environment) must enter
through declared shim interfaces, so a simulation harness can substitute
deterministic implementations. This is **not** a port of triglint — Python
has no MIR, no monomorphization, and no whole-program type information to
lean on — it is a new analysis that enforces the same *contract* with the
techniques Python affords.

The linter itself should be written in Rust (workspace crate, plausibly
`crates/trigpoint-pylint`), using an existing Rust Python parser (ruff's
`ruff_python_parser`/`ruff_python_ast` crates are the mature option) rather
than a Python-side plugin, so `trigp` stays one toolchain and one binary.

What carries over from triglint:

- The sink database concept: capability-labelled sinks matched by qualified
  name (`time.time`, `time.monotonic`, `random.*`, `os.urandom`,
  `secrets.*`, `uuid.uuid4`, `threading.*`, `socket.*`, `subprocess.*`,
  `os.environ`, file I/O). Python-specific sink: `set`/`frozenset`
  iteration order and `hash()` of `str`/`bytes` vary with
  `PYTHONHASHSEED` — the analog of triglint's `RandomState` type sink.
- Prod mode as the tractable v1: a per-module, syntactic scan for direct
  sink calls outside a blessed shim implementation. Shims are declared in
  config (a `Protocol`/ABC qualified name plus `grants`); "marked
  deterministic" maps to a marker base class or decorator shipped in a
  tiny dependency-free `trigpoint-shims` PyPI analog.
- The honesty principle: anything the analysis cannot resolve (dynamic
  attribute access, `getattr`, `importlib`, `eval`, monkeypatching) is
  reported as a hole, never silently assumed safe.

## Non-scope (at least initially)

- **Sim-mode whole-program reachability.** Sound call-graph construction
  for Python requires type inference the linter does not have. Options to
  evaluate later: consume type information from an existing checker
  (pyright/pyrefly/ty), or a conservative import-graph + name-resolution
  approximation with aggressive unresolved reporting. v1 is prod mode only.
- Runtime enforcement (import hooks, audit hooks). Worth considering as a
  *complement* — Python can enforce dynamically what it cannot prove
  statically — but it is a different tool.
- C extensions: opaque by definition; the module boundary is the fence,
  like triglint's FFI sink.

## Open questions

- Config: extend `triglint.toml` with a `[python]` section vs. a separate
  file. Leaning toward one config file for the whole workspace.
- CLI: `trigp lint` detecting Python sources vs. an explicit subcommand.
- How shim *consumption* is distinguished from shim *implementation*
  without types: probably "methods defined on a class that subclasses /
  is registered against a declared shim Protocol" — lexical, like
  triglint's prod-mode blessing.

## Related files

Nothing yet. First implementation steps: `crates/trigpoint-pylint` crate
with the ruff parser dependency, a Python sink database module, and a
fixture-based test suite mirroring `triglint/ui_prod/`.
