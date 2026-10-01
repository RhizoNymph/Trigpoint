# trigpylint — DST shim linting for Python codebases (v1)

Status: **implemented** as `crates/trigpoint-pylint`, wired into `trigp lint`.
Both modes ship: prod (`shim-nondeterminism`), sim (`sim-nondeterminism`), the
hole reporting (`unresolved`), the escape-hatch comment, and the `hash_order`
heuristic. Two pieces of the original plan are **not** in this crate and belong
to sibling workstreams: the shared `crates/trigpoint-config` extraction (step 1)
and the `trigpoint-shims` PyPI marker package (step 6). Where the analysis had
to diverge from this document, the divergence is recorded under
[Deviations from the design](#deviations-from-the-design) below.

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
- Diagnostics: `annotate-snippets`, pinned `= 0.12.16` (rustc's own renderer,
  maintained under rust-lang).
- Config: the `triglint.toml` schema grows a `[python]` section, defined in
  the shared stable crate `crates/trigpoint-config`
  (`trigpoint_config::python`) alongside the Rust lints' tables, so every
  consumer validates the whole file strictly — a typo in either half is an
  error everywhere, including a `[python]` typo surfacing through triglint.
  `crates/trigpoint-pylint/src/config.rs` re-exports the schema and adds
  what only the Python analysis needs: the `Resolved` query view (folding in
  the builtin Python sink database) and discovery anchored at a start
  directory (`$TRIGLINT_CONFIG`, else nearest `triglint.toml` walking up).
- Marker: a dependency-free PyPI package **`trigpoint-shims`** exporting
  `class DeterministicShim: ...` (and nothing else), mirroring the Rust
  crate's role: analyzed codebases depend on it solely to declare intent.
  Not shipped here; the linter only needs the qualified name
  `trigpoint_shims.DeterministicShim` to resolve through the binding table,
  which it does without the package being installed.

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
| net | module fences `socket`, `ssl`, `http`, `urllib`, `asyncio.streams`; `asyncio.open_connection`, `asyncio.start_server` (and their unix variants) |
| env | `os.environ`, `os.getenv`, `os.putenv`, module fences `platform`, `locale`; `sys.argv` |
| process | module fences `subprocess`, `signal`; `os.system`, `os.spawn*`, `os.exec*`, `os.getpid`, `os.getppid` |
| io | `input`, `sys.stdin` |
| extension | implicit: any sim-scope import that resolves to neither analyzed source nor an allow-listed/trusted module (the FFI analog — C extensions are opaque by definition) |

`print`/`sys.stdout` are deliberately not sinks (sim logging must work),
mirroring triglint.

Matching precedence is **exact call name > wildcard call name (`os.spawn*`) >
module fence**, and is independent of the order rules were declared in, so
adding a `[[python.sinks]]` entry can never reorder the builtin answers.

A small **builtin trusted-module set** (`sinks::builtin_trusted_modules`) keeps
imports of deterministic stdlib plumbing — `typing`, `dataclasses`, `json`,
`functools`, … — and of the modules that merely *host* call sinks (`time`,
`os`, `sys`, `datetime`, `io`, `uuid`) from being reported as opaque in sim
scope. Trusting a module never masks a sink inside it: sinks match on the name
at the use site, not on the import, exactly as triglint's
`BUILTIN_TRUSTED_CRATES` works.

## Analysis: data/control flow

1. **Config load** (`config::locate` → `config::parse_file`): `$TRIGLINT_CONFIG`
   or the nearest `triglint.toml` walking up; missing `[python]` section ⇒ the
   Python analysis is inert.
2. **Module collection** (`resolve::modules::collect`): walk `source_roots` for
   `.py` files; map file paths ↔ module paths (`src/myproj/sim/harness.py` ↔
   `myproj.sim.harness`, honoring `__init__.py`). Caches, virtualenvs and
   build output are skipped; a file whose name is not an identifier cannot be
   imported and is skipped; a file that fails to parse is reported as an
   `unresolved` warning rather than quietly dropped.
3. **Per-module pass** (parse once with `ruff_python_parser`, reuse for
   both modes):
   a. **Binding table** (`resolve::bindings`): `import x`, `import x as y`,
      `from x import y (as z)`, `from . import y` (resolved against the
      module's package), module-level aliases of the simple form
      `NAME = <resolvable dotted name>`, and module-level `def`/`class`
      statements (bound as `<module>.<name>`, which is how a shim protocol
      declared in its own module resolves). Star imports resolve through the
      target module's bindings when it is in-source, else are a hole; the
      merge iterates to a fixpoint so chained star imports settle.
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
      (the reify rule). A dotted chain is resolved and matched *as a whole*,
      so `random.random()` is one finding, not one per prefix. Arity-sensitive
      builtins (`time.localtime`, `time.gmtime`) use the call site's argument
      count, and a bare reference to one — where the argument list is not
      knowable — stays a sink. Module fences also match the `import`
      statement itself, **in sim mode only** (see Deviations).
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
   `# triglint: allow(<lint-name>)`, scanned from the source lines — the
   `#[allow]` analog. It is honoured on the finding's own line, on a
   standalone comment line directly above it, and on the `def`/`class` header
   line of an enclosing definition (which covers the whole definition).
   Exit code nonzero on any deny-level finding; `trigp` maps this like the
   Rust path.

Diagnostics have two renderings, both part of the contract:
`Diagnostic::summary()` is one stable machine-comparable line
(`<level> <lint> <file>:<line>:<col> <detail>`), and `Diagnostic::render()` is
the rustc-style snippet a human reads.

## CLI

`trigp lint` grows target detection: a `triglint.toml` with a `[python]`
section enables the Python analysis; a `Cargo.toml` at or above the directory
declaring `[workspace.metadata.dylint]`/`[package.metadata.dylint]` enables the
Rust one; both run when both apply. `--rust` / `--python` restrict explicitly.
The Python path needs none of the dylint environment folklore — no nightly, no
DYLINT_RUSTFLAGS, no subprocess — so orchestration is just config discovery +
a library call. A deny-level finding from either target fails the run; the
Rust exit code is preserved when it is the one that failed.

## Testing

- Golden-file fixtures mirroring `triglint/ui`: `fixtures/prod/*.py` (one flat
  package analyzed in a single run, each file with a `<stem>.expected`) and
  `fixtures/sim/<case>/` trees with their own `triglint.toml` and one
  `expected.txt`. The harness (`tests/fixtures.rs`) is custom — no
  snapshot-crate dependency — and `TRIGPYLINT_BLESS=1` rewrites goldens after
  a deliberate change.
- Goldens are written in the one-line `summary()` form so they can be authored
  and reviewed by hand; two additional goldens under `fixtures/render/` pin
  the `annotate-snippets` output itself so the human rendering cannot drift
  silently.
- Unit tests: binding-table resolution (aliases, relative imports, star
  imports, module-vs-value kinds, builtin shadowing), module-path mapping and
  package detection, config parsing (tolerance for Rust-side keys, rejection of
  `[python]` typos, discovery), sink matching precedence and arity, allow-comment
  parsing, line indexing, sim closure and witness chains, missing-root errors.
- Fixture inventory (written before implementation): prod — direct call,
  aliased import, `from` import, bare reference (reify), module-level alias,
  lambda in blessed method, nested def, module fence, dynamic-import/eval
  holes, dynamic-attribute hole, monkeypatch hole, star-import hole, blessed
  impl clean, wrong grant, marked impl violation, escape hatch; sim — clean
  closure, transitive violation with witness chain, import fence,
  relative-import resolution, set/hash_order heuristic, opaque import,
  marked-impl broken claim.
- Totals as shipped: 19 lib unit tests, 6 fixture-harness tests, 9 resolve
  integration tests in `trigpoint-pylint`, plus 9 in `trigpoint-cli`.

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
- One Python version's syntax per run — the parser's own latest supported
  version (`target_version` is not implemented, see Deviations); sinks behind
  `sys.version_info` branches are still scanned (no cfg-stripping — simpler
  and stricter than the Rust side).
- Diagnostics are deterministic: sorted by (file, offset, lint, detail) and
  de-duplicated, so two runs over the same tree produce byte-identical output.

## Deviations from the design

Recorded where implementation met Python or parser reality. In each case the
guarantee is kept honest — the choice reports more, or reports a hole, rather
than assuming safety.

1. **Import statements are fenced in sim mode only.** The design's step 3c said
   module fences "also match the `import` statement itself" for both modes. In
   prod mode the import sits at module level, where no class can bless it, so
   the rule would make a shim implementation module impossible to write: the
   very module that is supposed to import `time` could never do so. Prod mode
   therefore reports at the *use* sites — which, by the reify rule, already
   includes bare references, so nothing reachable escapes — and sim mode
   reports at the import. `prodcheck.rs` carries the same note.
2. **The `[python]` schema was born in this crate, then moved.** Step 1 of
   the plan was a sibling workstream, so `trigpoint-pylint` initially held
   the schema and parsed `triglint.toml` tolerantly (only `[python]` read,
   other top-level keys ignored). Since unification the schema lives in
   `trigpoint_config::python`, parsing goes through the full shared strict
   `Config`, and unknown top-level keys are errors for the Python path too.
3. **`asyncio` is not a blanket fence.** The design listed "`asyncio` streams"
   under `net`. A whole-module `asyncio` fence would swallow every sim harness
   that runs an event loop, and `asyncio.sleep` is already a `time` sink, so
   the fence is narrowed to `asyncio.streams` plus the four stream
   constructors.
4. **A builtin trusted-module set was added.** With `trusted_modules = []` as
   the only default, every `import typing` in sim scope became an `unresolved`
   warning and the honest-reporting signal drowned. The set covers
   deterministic stdlib plumbing and the modules that host call sinks; it
   never masks a sink, which still matches at the use site.
5. **`[python] target_version` is not implemented.** The crate calls
   `ruff_python_parser::parse_module`, which targets the parser's latest
   supported version. Adding the knob is a small follow-up.
6. **Imports are collected module-wide, not top-level only.** A function-local
   `import random` binds `random` for the whole module. Missing those would
   hide sinks; hoisting them can only surface more.
7. **Builtin shadowing is module-scoped, not scope-precise.** A name assigned
   anywhere in a module stops resolving to a builtin everywhere in that module,
   so a local named `open` silences the `open` sink for that file. This is the
   documented precision boundary for builtins; closing it needs real scope
   analysis.
8. **Blessing looks at direct bases only.** A class inheriting from an
   in-source class that itself implements a shim protocol is not blessed
   transitively. Declaring the protocol among the bases (the idiomatic
   `Protocol` style) is what the checker keys on.
9. **The escape comment accepts any emitted lint name.** The design named only
   `shim-nondeterminism`; `sim-nondeterminism` and `unresolved` are accepted
   too, comma-separated, since the same syntax has to be able to silence a sim
   finding or a hole.
10. **Sim roots that name no collected module are a hard error**, not a
    warning: a typo'd root would otherwise check nothing and report success.

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

## Implementation plan (status)

1. ✅ `crates/trigpoint-config`: extract triglint's config module into the
   stable workspace; repoint triglint (path dep); the `[python]` schema
   lives there too (`trigpoint_config::python`), typed, with no
   passthrough.
2. ✅ `crates/trigpoint-pylint` skeleton: module collection, binding tables,
   qualified-name resolution + unit tests.
3. ✅ Prod mode + fixture harness (fixtures written first).
4. ✅ Sim mode (import closure + witness chains) + fixtures.
5. ✅ `trigp lint` target detection + `--python`/`--rust`.
6. ✅ `trigpoint-shims` PyPI package (separate `python/` directory,
   pyproject.toml, no publish automation), together with
   `examples/py-demo/`, the end-to-end integration target the linter must
   keep clean.

## Related files

| file | role |
|---|---|
| `crates/trigpoint-pylint/Cargo.toml` | exact pins: `ruff_python_parser`/`ruff_python_ast`/`ruff_text_size` `= 0.0.13`, `annotate-snippets` `= 0.12.16` |
| `crates/trigpoint-pylint/src/lib.rs` | orchestration: config → resolve → prod/sim scans → diagnostics; `Analysis`, `analyze`, `run`, allow-comment suppression |
| `crates/trigpoint-pylint/src/config.rs` | re-exports the shared schema; `Resolved` query view, strict parsing via `trigpoint_config::Config`, discovery |
| `crates/trigpoint-pylint/src/resolve.rs` | `Program`: module table + one binding table per module |
| `crates/trigpoint-pylint/src/resolve/modules.rs` | `source_roots` → dotted module paths, parsing, relative-import arithmetic |
| `crates/trigpoint-pylint/src/resolve/bindings.rs` | binding tables, star-import fixpoint, `Expr` → qualified-name resolution |
| `crates/trigpoint-pylint/src/sinks.rs` | builtin Python sink database, matching precedence, trusted modules |
| `crates/trigpoint-pylint/src/prodcheck.rs` | blessing resolution + the shared per-module sink/hole scanner (`Policy::Prod` / `Policy::Sim`) |
| `crates/trigpoint-pylint/src/simscope.rs` | import graph, BFS closure, witness chains, opaque-import holes |
| `crates/trigpoint-pylint/src/diagnostics.rs` | `Lint`/`Level`/`Diagnostic`, annotate-snippets rendering, line index, allow-comment handling |
| `crates/trigpoint-pylint/fixtures/prod/` | flat prod corpus, one `.expected` per `.py` |
| `crates/trigpoint-pylint/fixtures/sim/<case>/` | sim package trees, one `expected.txt` per case |
| `crates/trigpoint-pylint/fixtures/render/` | pinned `annotate-snippets` renderings |
| `crates/trigpoint-pylint/tests/fixtures.rs` | golden-file harness (`TRIGPYLINT_BLESS=1` to regenerate) |
| `crates/trigpoint-pylint/tests/resolve.rs` | module-mapping, binding and closure integration tests |
| `crates/trigpoint-cli/src/lint.rs` | `trigp lint` args, target detection, unified exit code |
| `crates/trigpoint-cli/src/lint/python.rs` | the Python target: analyze, print, decide the exit code |
| `crates/trigpoint-cli/src/lint/dylint.rs` | the Rust target: cargo-dylint orchestration, dylint-metadata detection |
| `crates/trigpoint-config` | shared `triglint.toml` schema: the Rust tables (from `triglint/src/config.rs`) and the `[python]` section (`src/python.rs`) |
| `python/trigpoint-shims/` | PyPI marker package (`DeterministicShim`): hatchling, no deps, no publish automation |
| `examples/py-demo/` | end-to-end integration target — `ClockShim` protocol, marked `SimClock`, blessed `SystemClock` in `pydemo.prod`, quarantined `pydemo.violate`, `[python]` triglint.toml |
