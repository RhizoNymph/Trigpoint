//! `trigpylint` — static DST shim-contract linting for Python sources.
//!
//! Two checks, mirroring triglint's split:
//!
//! * **prod mode** (`shim-nondeterminism`, deny) — a nondeterminism sink may
//!   only be *named*, called or merely referenced, inside a class implementing
//!   a declared shim protocol whose grants cover the capability.
//! * **sim mode** (`sim-nondeterminism`, deny) — from the declared simulation
//!   root modules, no module in the transitive import closure names a sink at
//!   all, blessed or not.
//!
//! What the analysis cannot see it reports (`unresolved`, warn) rather than
//! assuming safe: dynamic imports, `eval`/`exec`, non-literal `getattr` on
//! module objects, monkeypatching, star imports from outside the source tree,
//! and imports inside sim scope that resolve to neither analyzed source nor a
//! trusted module.
//!
//! The pipeline is: [`config`] → [`resolve`] (module table + binding tables) →
//! [`prodcheck`] (per-module sink/hole scan, blessing-aware) and [`simscope`]
//! (import closure + witness chains) → [`diagnostics`].

pub mod config;
pub mod diagnostics;
pub mod prodcheck;
pub mod resolve;
pub mod simscope;
pub mod sinks;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use thiserror::Error;

use config::{PythonConfig, Resolved};
use diagnostics::{AllowComments, Diagnostic, Level, LineIndex, Lint};
use prodcheck::{Context, Finding, FindingKind, Hole, Policy};
use resolve::{Module, Program};
use sinks::SinkKind;

#[derive(Debug, Error)]
pub enum PylintError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(transparent)]
    Resolve(#[from] resolve::ResolveError),
    #[error("[python.sim] roots name modules that were not collected: {}", .roots.join(", "))]
    MissingSimRoots { roots: Vec<String> },
}

/// The result of one run.
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    /// Modules collected from `source_roots`.
    pub modules_collected: usize,
    /// Modules inside the sim import closure (0 when sim mode is off).
    pub sim_modules: usize,
    sources: BTreeMap<String, String>,
}

impl Analysis {
    pub fn denials(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level() == Level::Deny)
            .count()
    }

    pub fn has_denials(&self) -> bool {
        self.denials() > 0
    }

    /// The stable one-line-per-finding form used by the fixture goldens.
    pub fn summaries(&self) -> Vec<String> {
        self.diagnostics.iter().map(Diagnostic::summary).collect()
    }

    /// The human-facing rendering of every diagnostic, in order.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for diagnostic in &self.diagnostics {
            let source = self
                .sources
                .get(&diagnostic.display_path)
                .map_or("", String::as_str);
            out.push_str(&diagnostic.render(source));
            out.push('\n');
        }
        out
    }

    /// Diagnostics for one file, in source order.
    pub fn for_file(&self, display_path: &str) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(move |d| d.display_path == display_path)
    }
}

/// Locates and loads the config, then analyzes. `Ok(None)` means there is no
/// `triglint.toml` with a `[python]` section anywhere above `start_dir`, in
/// which case the Python analysis is inert.
pub fn run(start_dir: &Path) -> Result<Option<Analysis>, PylintError> {
    let Some(config_path) = config::locate(start_dir) else {
        return Ok(None);
    };
    let Some(python) = config::parse_file(&config_path)? else {
        return Ok(None);
    };
    let root = config_path.parent().unwrap_or(Path::new(".")).to_owned();
    analyze(&root, python).map(Some)
}

/// Analyzes the project rooted at `root_dir` (the directory holding the config
/// file) under the given `[python]` configuration.
pub fn analyze(root_dir: &Path, python: PythonConfig) -> Result<Analysis, PylintError> {
    let config = Resolved::new(root_dir, python);
    let program = Program::load(root_dir, config.source_roots())?;

    let mut builder = Builder::default();
    for unparsed in program.modules.unparsed() {
        builder.push(Diagnostic {
            lint: Lint::Unresolved,
            path: unparsed.path.clone(),
            display_path: unparsed.display_path.clone(),
            range: ruff_text_size::TextRange::default(),
            line: 1,
            column: 1,
            title: format!("{} could not be parsed", unparsed.display_path),
            label: "parse error".to_owned(),
            notes: vec![format!("parser said: {}", unparsed.error)],
            detail: "parse-error".to_owned(),
        });
    }

    let mut sim_modules = 0;
    if config.sim_enabled() {
        let closure = simscope::closure(&program, &config)
            .map_err(|missing| PylintError::MissingSimRoots { roots: missing.0 })?;
        sim_modules = closure.order.len();
        for id in &closure.order {
            let module = program.modules.module(*id);
            let witness = closure.witness(*id);
            let findings = prodcheck::scan(module, program.bindings(*id), &config, Policy::Sim);
            emit(
                &mut builder,
                module,
                findings,
                Mode::Sim { witness: &witness },
            );
        }
        for unresolved in &closure.unresolved {
            let module = program.modules.module(unresolved.module);
            let witness = closure.witness(unresolved.module);
            let (line, column) = position(module, unresolved.range.start());
            builder.push(Diagnostic {
                lint: Lint::Unresolved,
                path: module.path.clone(),
                display_path: module.display_path.clone(),
                range: unresolved.range,
                line,
                column,
                title: format!(
                    "import of `{}` inside simulation scope cannot be analyzed",
                    unresolved.target
                ),
                label: "no analyzed source for this module".to_owned(),
                notes: vec![
                    format!("witness: {witness}"),
                    format!(
                        "add it to [python] source_roots, or trust it with [python.opaque] trusted_modules = [\"{}\"]",
                        unresolved.target
                    ),
                ],
                detail: format!("opaque-import {} witness={}", unresolved.target, witness),
            });
        }
    }

    if config.prod_enabled() {
        for (module, bindings) in program.iter() {
            let findings = prodcheck::scan(module, bindings, &config, Policy::Prod);
            emit(&mut builder, module, findings, Mode::Prod);
        }
    }

    let sources = program
        .modules
        .iter()
        .map(|module| (module.display_path.clone(), module.source.clone()))
        .collect();
    let mut diagnostics = builder.finish();
    suppress(&mut diagnostics, &program);

    Ok(Analysis {
        diagnostics,
        modules_collected: program.modules.len(),
        sim_modules,
        sources,
    })
}

enum Mode<'a> {
    Prod,
    Sim { witness: &'a str },
}

#[derive(Default)]
struct Builder {
    diagnostics: Vec<Diagnostic>,
    seen: BTreeSet<(Lint, String, u32, u32, String)>,
}

impl Builder {
    fn push(&mut self, diagnostic: Diagnostic) {
        let key = (
            diagnostic.lint,
            diagnostic.display_path.clone(),
            u32::from(diagnostic.range.start()),
            u32::from(diagnostic.range.end()),
            diagnostic.detail.clone(),
        );
        if self.seen.insert(key) {
            self.diagnostics.push(diagnostic);
        }
    }

    fn finish(mut self) -> Vec<Diagnostic> {
        self.diagnostics.sort_by(|a, b| {
            (
                &a.display_path,
                u32::from(a.range.start()),
                a.lint,
                &a.detail,
            )
                .cmp(&(
                    &b.display_path,
                    u32::from(b.range.start()),
                    b.lint,
                    &b.detail,
                ))
        });
        self.diagnostics
    }
}

fn position(module: &Module, offset: ruff_text_size::TextSize) -> (u32, u32) {
    LineIndex::new(&module.source).line_column(&module.source, offset)
}

fn emit(builder: &mut Builder, module: &Module, findings: Vec<Finding>, mode: Mode<'_>) {
    let index = LineIndex::new(&module.source);
    for finding in findings {
        let (line, column) = index.line_column(&module.source, finding.range.start());
        let Some((lint, title, label, mut notes, detail)) = describe(&finding, &mode) else {
            continue;
        };
        if let Mode::Sim { witness } = mode
            && lint != Lint::Unresolved
        {
            notes.push(format!("witness: {witness}"));
        }
        builder.push(Diagnostic {
            lint,
            path: module.path.clone(),
            display_path: module.display_path.clone(),
            range: finding.range,
            line,
            column,
            title,
            label,
            notes,
            detail,
        });
    }
}

type Described = (Lint, String, String, Vec<String>, String);

fn describe(finding: &Finding, mode: &Mode<'_>) -> Option<Described> {
    match &finding.kind {
        FindingKind::Sink {
            capability,
            name,
            matched,
            context,
        } => Some(match mode {
            Mode::Prod => {
                let (context_note, context_detail) = render_context(context);
                (
                    Lint::ShimNondeterminism,
                    format!(
                        "nondeterminism sink `{name}` ({capability}) named outside a shim implementation"
                    ),
                    format!("{capability} sink named here"),
                    vec![
                        context_note,
                        matched_note(matched),
                        "route this through a shim protocol implementation, or grant the capability to the enclosing one".to_owned(),
                    ],
                    format!("{capability} {name} {context_detail}"),
                )
            }
            Mode::Sim { witness } => {
                let mut notes = vec![matched_note(matched)];
                let mut detail = format!("{capability} {name} witness={witness}");
                if let Context::MarkedDeterministic { class } = context {
                    notes.push(format!(
                        "`{class}` is marked deterministic: naming a sink here is a broken determinism claim"
                    ));
                    detail.push_str(&format!(" broken-claim:{class}"));
                }
                (
                    Lint::SimNondeterminism,
                    format!(
                        "nondeterminism sink `{name}` ({capability}) is reachable from a simulation root"
                    ),
                    format!("{capability} sink named here"),
                    notes,
                    detail,
                )
            }
        }),
        FindingKind::ImportFence { capability, module } => {
            let Mode::Sim { witness } = mode else {
                return None;
            };
            Some((
                Lint::SimNondeterminism,
                format!("simulation scope imports the fenced module `{module}` ({capability})"),
                format!("{capability} module imported here"),
                vec![
                    "the whole module is fenced: importing it puts its surface in simulation scope"
                        .to_owned(),
                ],
                format!("{capability} import:{module} witness={witness}"),
            ))
        }
        FindingKind::HashOrder { construct, context } => {
            let Mode::Sim { witness } = mode else {
                return None;
            };
            let mut notes = vec![
                "set iteration order varies with PYTHONHASHSEED; this is a syntactic heuristic"
                    .to_owned(),
                "sort before iterating, or run simulations with PYTHONHASHSEED=0".to_owned(),
            ];
            let mut detail = format!("hash_order {construct} witness={witness}");
            if let Context::MarkedDeterministic { class } = context {
                notes.push(format!("`{class}` is marked deterministic"));
                detail.push_str(&format!(" broken-claim:{class}"));
            }
            Some((
                Lint::SimNondeterminism,
                format!("{construct} in simulation scope has PYTHONHASHSEED-dependent order"),
                "hash_order sink constructed here".to_owned(),
                notes,
                detail,
            ))
        }
        FindingKind::Hole(hole) => Some(describe_hole(hole)),
    }
}

fn describe_hole(hole: &Hole) -> Described {
    let (title, label, note, detail) = match hole {
        Hole::DynamicImport { name } => (
            format!("`{name}` imports a module the analysis cannot name"),
            "dynamic import".to_owned(),
            "the imported module is not known statically, so its sinks are invisible".to_owned(),
            format!("dynamic-import {name}"),
        ),
        Hole::DynamicEval { name } => (
            format!("`{name}` executes code the analysis cannot see"),
            "dynamic evaluation".to_owned(),
            "anything reachable from the evaluated source is outside the guarantee".to_owned(),
            format!("dynamic-eval {name}"),
        ),
        Hole::DynamicAttribute { function, module } => (
            format!("`{function}` on module `{module}` with a non-literal attribute"),
            "dynamic attribute access".to_owned(),
            "the attribute is not known statically, so it cannot be matched against the sink database"
                .to_owned(),
            format!("dynamic-attribute {function}({module})"),
        ),
        Hole::Monkeypatch { target } => (
            format!("assignment to `{target}` replaces a module attribute"),
            "monkeypatch".to_owned(),
            "the analysis judged the original attribute; after this it is something else".to_owned(),
            format!("monkeypatch {target}"),
        ),
        Hole::StarImport { target } => (
            format!("`from {target} import *` has no analyzed source"),
            "star import".to_owned(),
            "names it introduces cannot be resolved, so sinks reached through them are invisible"
                .to_owned(),
            format!("star-import {target}"),
        ),
    };
    (Lint::Unresolved, title, label, vec![note], detail)
}

fn render_context(context: &Context) -> (String, String) {
    match context {
        Context::Unblessed => (
            "not inside any shim protocol implementation".to_owned(),
            "unblessed".to_owned(),
        ),
        Context::MarkedDeterministic { class } => (
            format!("`{class}` is marked deterministic and receives no grants"),
            format!("marked:{class}"),
        ),
        Context::GrantsMissing { protocol, grants } => (
            format!(
                "the enclosing implementation of `{protocol}` grants [{}]",
                grants.join(", ")
            ),
            format!("grants-missing:{protocol}[{}]", grants.join(",")),
        ),
    }
}

fn matched_note(matched: &SinkKind) -> String {
    match matched {
        SinkKind::Call(pattern) => format!("matched the call sink `{pattern}`"),
        SinkKind::ModuleFence(module) => format!("matched the module fence `{module}`"),
    }
}

/// Drops diagnostics covered by a `# triglint: allow(...)` comment.
fn suppress(diagnostics: &mut Vec<Diagnostic>, program: &Program) {
    let allows: BTreeMap<&str, (AllowComments, LineIndex)> = program
        .modules
        .iter()
        .map(|module| {
            let index = LineIndex::new(&module.source);
            let allow = AllowComments::scan(&module.source, module.ast(), &index);
            (module.display_path.as_str(), (allow, index))
        })
        .collect();
    diagnostics.retain(|diagnostic| {
        allows
            .get(diagnostic.display_path.as_str())
            .is_none_or(|(allow, _)| {
                !allow.suppresses(diagnostic.lint, diagnostic.line, diagnostic.range.start())
            })
    });
}
