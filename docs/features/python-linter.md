# trigpylint — DST shim linting for Python codebases (design v1, not implemented)

Status: **designed, not implemented.** This doc is the implementation
contract for the workstream; update it as reality diverges.

## Scope

Bring the trigpoint DST contract to Python codebases: all nondeterminism
(time, randomness, I/O, threads, subprocesses, environment) must enter
through declared shim protocols, so a simulation harness can substitute
deterministic implementations. This is **not** a port of triglint — Python
has no MIR, no monomorphization, and no whole-program type information —
it is a new analysis enforcing the same *contract* with the techniques
Python affords.

Two lints, mirroring triglint's split:

**Prod mode** (`shim_nondeterminism` analog, deny):

> A nondeterminism sink may only be *named* — called or referenced —
> inside a method of a class implementing a declared shim protocol whose
> grants cover the capability.

Introduction points in Python are name references. A sink callable can be
reached three ways: (a) a resolvable name chain (`time.time()`,
`from time import monotonic; monotonic()`) — matched against the sink
database; (b) dynamic access (`getattr`, `importlib`, `__import__`,
`eval`/`exec`) — flagged as holes; (c) aliasing through data flow
(`f = time.time; ...; f()`). Case (c) is closed the same way triglint v1.2
closes function pointers: the *bare reference* to a sink (no call) is
where the indirection is created, and it is a violation at that site.
Together (a)+(b)+(c) mean a sink cannot be reached from checked code
without tripping one of the three — that is the soundness story, stated
per-module with no cross-module data flow needed.

**Sim mode** (`sim_nondeterminism` analog, deny):

> From the declared simulation root modules, no module in the transitive
> import closure names a sink at all — including inside shim impls; a
> marked-deterministic impl that does is a broken claim.

Reachability is over the *import graph*, not a call graph: module-level
`import`/`from` edges from the roots, resolved against configured source
roots. This is deliberately over-approximate — importing a module pulls
its whole surface into sim scope even if only one function is called —
matching triglint v1.2's philosophy (a collected vtable method counts even
if never invoked). Sink *modules* imported inside sim scope
(`import random`) violate at the import statement; unresolvable imports
(C extensions, unvendored third-party) are holes unless trusted.

Like triglint, honesty is the invariant: what the analysis cannot see is
reported as an `unresolved` warning, never assumed safe. In Python the
holes are dynamic import, `eval`/`exec`, `getattr` with a non-literal
attribute, and monkeypatching (assignment to attributes of imported
modules), each reported at its site within checked scope.

## Non-scope (deferred or rejected)

- **Call-graph sim mode.** Sound Python call graphs need type inference.
  Astral's `ty` is the obvious future substrate but is alpha and its
  engine (like `ruff_python_semantic`) is not published to crates.io.
  Revisit when it stabilizes; the import-closure mode stands on its own.
- **Cross-module data-flow tracking.** The sink-reference rule makes it
  unnecessary for the guarantee (see soundness story above).
- **Runtime enforcement.** Temporal's Python SDK workflow sandbox proves
  the runtime-proxy approach works (custom importer + proxy modules that
  raise on nondeterministic calls) and is the model to steal from if
  trigpoint ever grows a runtime harness; it is a different tool, and
  static checking is complementary (no runtime cost, covers unexecuted
  paths).
- **Heuristic type-dependent sinks.** `set`/`frozenset` iteration order
  and `hash(str)` vary with PYTHONHASHSEED (Python's RandomState analog;
  `dict` is insertion-ordered and fine). Without types we can only catch
  syntactic constructions — set literals, `set()`/`frozenset()` calls,
  set comprehensions — in sim scope. Shipped as a best-effort
  `hash_order` capability, on by default in sim scope only, documented as
  heuristic. Operational backstop: run sims with `PYTHONHASHSEED=0`.
- **Address nondeterminism** (`id()`, `object.__repr__` addresses): same
  deferral as triglint's pointer-address non-scope.
- **Attribute-call matching on unknown receivers** (`obj.read()`): the
  norm in Python, not reportable as holes without drowning the user;
  sinks are matched on resolvable qualified names only. This is the
  documented precision boundary.

## Toolchain and dependency model

- New **stable-workspace** crate `crates/trigpoint-pylint` (library) wired
  into `trigp` — no nightly, no dylint; plain Rust.
- Parser: `ruff_python_parser` + `ruff_python_ast`, pinned **exactly**
  (`= 0.0.13`) — they are published weekly as 0.0.x internal component
  crates of ruff with no API-stability promise, so upgrades are deliberate
  and reviewed. (`rustpython-parser` rejected: lags modern syntax such as
  PEP 701 f-strings and PEP 695 type parameters.)
- `ruff_python_semantic` is **not** published; binding/name resolution is
  implemented in-house (`resolve.rs`) — for this analysis it is small:
  per-module import/alias tables, not a type system.
- Config: the `triglint.toml` schema grows a `[python]` section. Because
  triglint's `Config` uses `deny_unknown_fields`, the schema moves to a
  shared stable crate `crates/trigpoint-config` (triglint's `config.rs`
  is already deliberately rustc-free, so it can move as-is); the triglint
  workspace path-depends on it. One config file, two consumers.
- Marker: a dependency-free PyPI package **`trigpoint-shims`** exporting
  `class DeterministicShim: ...` (and nothing else), mirroring the Rust
  crate's role: analyzed codebases depend on it solely to declare intent.

## Configuration: `triglint.toml` `[python]` section

```toml
[python]
# Import-resolution roots, relative to the config file.
source_roots = ["src"]

[python.sim]
# Module paths of simulation harness entry modules.
roots = ["myproj.sim.harness"]

[[python.shims]]
protocol = "myproj.shims.ClockShim"   # qualified class name
grants = ["time"]

[python.markers]
deterministic = ["trigpoint_shims.DeterministicShim"]

# Additional sinks, merged with builtins (builtin_sinks = false to drop).
[[python.sinks]]
capability = "time"
calls = ["arrow.utcnow"]              # exact qualified-name match
modules = ["pendulum"]                # whole-module fence (import or use)

[python.opaque]
trusted_modules = []                  # imports allowed without source
allow = []                            # qualified names permitted dynamic/opaque
```

Discovery is identical to triglint: nearest `triglint.toml` walking up,
`TRIGLINT_CONFIG` env override. Presence of `[[python.shims]]` enables
prod mode; presence of `[python.sim] roots` enables sim mode.

## Builtin sink database (Python)

Defined at the stdlib boundary, one namespace-qualified name per entry:

| capability | sinks |
|---|---|
| time | `time.time`, `time.time_ns`, `time.monotonic(_ns)`, `time.perf_counter(_ns)`, `datetime.datetime.now`, `datetime.datetime.utcnow`, `datetime.datetime.today`, `datetime.date.today`, no-arg `time.localtime`/`time.gmtime`, `asyncio.sleep`, `time.sleep` |
| random | module fences `random`, `secrets`, `numpy.random`; `os.urandom`, `uuid.uuid1`, `uuid.uuid4` |
| hash_order | heuristic type sinks: set literals, `set`/`frozenset` calls and comprehensions (sim scope only, see non-scope) |
| thread | module fences `threading`, `multiprocessing`, `concurrent.futures`; `os.fork` |
| fs | `open`, `io.open`, module fences `pathlib`, `shutil`, `tempfile`, `glob`; `os.listdir`, `os.scandir`, `os.walk` (ordering is fs-dependent) |
| net | module fences `socket`, `ssl`, `http`, `urllib`, `asyncio` streams |
| env | `os.environ`, `os.getenv`, module fences `platform`, `locale`; `sys.argv` |
| process | module fence `subprocess`; `os.system`, `os.spawn*`, `os.exec*`, `os.getpid`, module fence `signal` |
| io | `input`, `sys.stdin` |
| extension | implicit: any sim-scope import that resolves to neither analyzed source nor an allow-listed/trusted module (the FFI analog — C extensions are opaque by definition) |

`print`/`sys.stdout` are deliberately not sinks (sim logging must work),
mirroring triglint.

## Analysis: data/control flow

1. **Config load**: shared `trigpoint-config` crate; missing `[python]`
   section ⇒ the Python analysis is inert.
2. **Module collection**: walk `source_roots` for `.py` files; map file
   paths ↔ module paths (`src/myproj/sim/harness.py` ↔
   `myproj.sim.harness`, honoring `__init__.py`).
3. **Per-module pass** (parse once with `ruff_python_parser`, reuse for
   both modes):
   a. **Binding table**: `import x`, `import x as y`,
      `from x import y (as z)`, `from . import y` (resolved against the
      module's package), plus module-level aliases of the simple form
      `NAME = <resolvable dotted name>`. Star imports resolve through the
      target module's bindings when it is in-source, else are a hole.
   b. **Blessing resolution**: for each `class` statement, resolve base
      names through the binding table; a class whose bases include a
      declared shim protocol is blessed with its grants; bases including
      a deterministic marker ⇒ marked (no grants, broken-claim
      attribution). Functions walk lexically outward through nested
      defs/lambdas/comprehensions to the nearest class or module scope —
      the `non_closure_owner` analog.
   c. **Sink scan**: every `Name`/`Attribute` expression is resolved to a
      qualified name where possible and matched against the database
      (exact call name, or module fence on the leading segment). A match
      is a violation if the enclosing blessing does not cover the
      capability — whether the expression is called or merely referenced
      (the reify rule). Module fences also match the `import` statement
      itself.
   d. **Hole scan**: `importlib.*`, `__import__`, `eval`, `exec`,
      `getattr`/`setattr` with non-literal attribute on a resolved module
      object, and assignment targets that are attributes of imported
      modules (monkeypatching) ⇒ unresolved warnings.
4. **Prod mode** (shims declared): steps 3b–3d over every collected
   module; diagnostics carry the blessing context (unblessed / marked /
   grants-missing), same taxonomy as triglint's `prodcheck`.
5. **Sim mode** (roots declared): BFS over import edges from the root
   modules; for each module in the closure, run the rule "zero sinks
   allowed regardless of blessing" (marked impls annotate the diagnostic
   as a broken determinism claim). The witness chain is the import chain
   root → … → module plus the sink site — reconstructed from a parent map
   exactly like triglint's frame tree. Holes inside the closure are
   warnings; outside it, silent.
6. **Diagnostics**: rendered with `annotate-snippets` (rustc-style,
   maintained under rust-lang) with capability label, witness chain, and
   fix guidance (route through a shim / trust the module / escape hatch).
   Escape hatch is a line or def-level comment
   `# triglint: allow(shim-nondeterminism)`, scanned from the token
   stream — the `#[allow]` analog. Exit code nonzero on any deny-level
   finding; `trigp` maps this like the Rust path.

## CLI

`trigp lint` grows target detection: a workspace with `[python]` config
runs the Python analysis; with dylint metadata it runs cargo-dylint; both
when both. `--rust` / `--python` restrict explicitly. The Python path
needs none of the dylint environment folklore — no nightly, no
DYLINT_RUSTFLAGS — so orchestration is just config discovery + invocation.

## Testing

- Golden-file fixtures mirroring `triglint/ui`: `fixtures/prod/*.py` and
  `fixtures/sim/<case>/` trees (sim cases need multi-module packages),
  each with an expected-diagnostics file; a small custom harness compares
  rendered output (no snapshot-crate dependency).
- Unit tests: binding-table resolution (aliases, relative imports, star
  imports), module-path mapping, config parsing (shared crate), sink
  matching precedence.
- Fixture inventory to write before implementation (tests first): direct
  call, aliased import, `from` import, bare reference (reify), lambda in
  blessed method, nested def, module fence via import, dynamic-import
  hole, monkeypatch hole, blessed impl clean, wrong grant, marked impl
  violation, sim closure transitive violation, sim import-fence, sim
  clean, relative-import resolution, set-literal heuristic.

## Invariants and constraints

- Sink matching happens on resolvable qualified names only; anything
  dynamic in checked scope is a reported hole, never silently skipped —
  the honesty invariant carries over verbatim.
- A sink *reference* is a violation independent of a call: aliasing can
  never launder a sink through data flow without a flagged site.
- Blessing is lexical (nearest enclosing class through nested scopes) and
  capability-specific; deterministic-marked classes annul all grants.
- `trigpoint-shims` (PyPI) must stay dependency-free; importing it costs
  analyzed projects nothing.
- Parser crates are pinned exactly; upgrades are their own commits.
- No violation and no unresolved warning in sim scope ⇒ every import edge
  from the roots was resolved into analyzed-or-trusted modules and every
  name reference in the closure was sink-free — the evidence statement
  this linter contributes to trigpoint's bookkeeping.
- One Python version's syntax per run (parser target version from
  `[python] target_version`, default latest supported); sinks behind
  `sys.version_info` branches are still scanned (no cfg-stripping —
  simpler and stricter than the Rust side).

## Open questions

- Whether `[[python.shims]]` should also accept function-level shims
  (module of blessed free functions) — Rust forces traits; Python
  codebases often use plain modules as seams. Leaning yes, via
  `functions = ["myproj.shims.clock_now"]` grants, but deferred until a
  real codebase demands it.
- numpy/pandas ecosystem fences beyond `numpy.random` — driven by the
  first real analyzed project rather than speculation.
- Whether sim mode should eventually consume `ty`'s resolved semantics
  for call-level precision once Astral publishes consumable crates.

## Implementation plan (ordered)

1. `crates/trigpoint-config`: extract triglint's config module into the
   stable workspace; add the `[python]` schema; repoint triglint
   (path dep) — its UI tests must stay green untouched.
2. `crates/trigpoint-pylint` skeleton: module collection, binding tables,
   qualified-name resolution + unit tests.
3. Prod mode + fixture harness (fixtures written first).
4. Sim mode (import closure + witness chains) + fixtures.
5. `trigp lint` target detection + `--python`/`--rust`.
6. `trigpoint-shims` PyPI package (separate `python/` directory,
   pyproject.toml, no publish automation yet).

## Related files (planned)

| file | role |
|---|---|
| `crates/trigpoint-config` | shared `triglint.toml` schema (moved from `triglint/src/config.rs`) |
| `crates/trigpoint-pylint/src/lib.rs` | orchestration: collect → resolve → check |
| `crates/trigpoint-pylint/src/resolve.rs` | module mapping, binding tables, qualified-name resolution |
| `crates/trigpoint-pylint/src/sinks.rs` | builtin Python sink database |
| `crates/trigpoint-pylint/src/prodcheck.rs` | blessing resolution + per-module sink/hole scan |
| `crates/trigpoint-pylint/src/simscope.rs` | import graph, closure, witness chains |
| `crates/trigpoint-pylint/src/diagnostics.rs` | annotate-snippets rendering, allow-comment handling |
| `crates/trigpoint-pylint/fixtures/` | golden-file test corpus |
| `python/trigpoint-shims/` | PyPI marker package (`DeterministicShim`) |
