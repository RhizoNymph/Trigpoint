//! The Python nondeterminism sink database.
//!
//! Sinks are matched on *resolvable qualified names* only — `time.time`,
//! `datetime.datetime.now`, `random.randint`. Two shapes exist:
//!
//! * **call sinks**: an exact qualified name, or a `foo.bar*` wildcard for
//!   families such as `os.spawn*`. A few are arity-sensitive (`time.localtime`
//!   is deterministic when given an explicit timestamp), which the [`Usage`] of
//!   the site decides.
//! * **module fences**: a whole module (and everything under it) is a sink, so
//!   `random.Random`, `random.randint` and — inside sim scope — the statement
//!   `import random` itself all match.
//!
//! Matching precedence is exact call > wildcard call > module fence, and is
//! independent of the order rules were declared in, so adding a user sink can
//! never reorder the builtin answers.
//!
//! `print`/`sys.stdout` are deliberately *not* sinks: simulation logging has to
//! work. This mirrors triglint's Rust database.

use crate::config::SinkSpec;

/// How many arguments a call sink tolerates before it stops being a sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// A sink however it is called.
    Any,
    /// A sink only when called with no arguments (or merely referenced, where
    /// the argument list is not knowable).
    NoArgs,
}

/// Qualified-name matcher for a call sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamePattern {
    /// `time.time` matches exactly `time.time`.
    Exact(String),
    /// `os.spawn` matches `os.spawnl`, `os.spawnvpe`, … (a literal prefix, not
    /// a dotted-segment prefix — that is what module fences are for).
    Wildcard(String),
}

impl NamePattern {
    fn matches(&self, qualified: &str) -> bool {
        match self {
            NamePattern::Exact(name) => qualified == name,
            NamePattern::Wildcard(prefix) => qualified.starts_with(prefix.as_str()),
        }
    }

    fn rendered(&self) -> String {
        match self {
            NamePattern::Exact(name) => name.clone(),
            NamePattern::Wildcard(prefix) => format!("{prefix}*"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CallSink {
    pub pattern: NamePattern,
    pub arity: Arity,
}

impl CallSink {
    fn exact(name: &str) -> Self {
        Self {
            pattern: NamePattern::Exact(name.to_owned()),
            arity: Arity::Any,
        }
    }

    fn wildcard(prefix: &str) -> Self {
        Self {
            pattern: NamePattern::Wildcard(prefix.to_owned()),
            arity: Arity::Any,
        }
    }

    fn no_args(name: &str) -> Self {
        Self {
            pattern: NamePattern::Exact(name.to_owned()),
            arity: Arity::NoArgs,
        }
    }

    fn admits(&self, usage: Usage) -> bool {
        match self.arity {
            Arity::Any => true,
            // A bare reference cannot be shown to be the safe arity, so it
            // stays a sink: honesty over convenience.
            Arity::NoArgs => match usage {
                Usage::Referenced => true,
                Usage::Called { arguments } => arguments == 0,
            },
        }
    }
}

/// One capability's worth of sinks.
#[derive(Debug, Clone)]
pub struct SinkRule {
    pub capability: String,
    pub calls: Vec<CallSink>,
    pub modules: Vec<String>,
}

impl SinkRule {
    /// Lifts a user-declared `[[python.sinks]]` entry into a rule. User sinks
    /// are never arity-sensitive — that subtlety is reserved for the builtins,
    /// where it is documented per entry.
    pub fn from_spec(spec: &SinkSpec) -> Self {
        Self {
            capability: spec.capability.clone(),
            calls: spec.calls.iter().map(|c| CallSink::exact(c)).collect(),
            modules: spec.modules.clone(),
        }
    }
}

/// How the name under examination appears at its site. The reify rule makes a
/// bare reference a sink just like a call, so this only decides arity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usage {
    Called { arguments: usize },
    Referenced,
}

/// Why a qualified name matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkKind {
    /// Matched a call-sink pattern; the string is the pattern as written.
    Call(String),
    /// Matched a module fence; the string is the fenced module.
    ModuleFence(String),
}

impl SinkKind {
    pub fn rendered(&self) -> &str {
        match self {
            SinkKind::Call(pattern) => pattern,
            SinkKind::ModuleFence(module) => module,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkHit {
    pub capability: String,
    pub kind: SinkKind,
}

/// The queryable sink database.
#[derive(Debug)]
pub struct SinkDb {
    rules: Vec<SinkRule>,
}

impl SinkDb {
    pub fn new(rules: Vec<SinkRule>) -> Self {
        Self { rules }
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Matches a resolved qualified name. Precedence: exact call, then
    /// wildcard call, then module fence.
    pub fn match_name(&self, qualified: &str, usage: Usage) -> Option<SinkHit> {
        for exact_pass in [true, false] {
            for rule in &self.rules {
                for call in &rule.calls {
                    let is_exact = matches!(call.pattern, NamePattern::Exact(_));
                    if is_exact != exact_pass {
                        continue;
                    }
                    if call.pattern.matches(qualified) && call.admits(usage) {
                        return Some(SinkHit {
                            capability: rule.capability.clone(),
                            kind: SinkKind::Call(call.pattern.rendered()),
                        });
                    }
                }
            }
        }
        self.match_module(qualified)
    }

    /// Matches a module (or any name under it) against the fences only. Used
    /// for `import` statements in sim scope, where there is no call site.
    pub fn match_module(&self, module: &str) -> Option<SinkHit> {
        for rule in &self.rules {
            for fence in &rule.modules {
                if module == fence || module.starts_with(&format!("{fence}.")) {
                    return Some(SinkHit {
                        capability: rule.capability.clone(),
                        kind: SinkKind::ModuleFence(fence.clone()),
                    });
                }
            }
        }
        None
    }
}

fn rule(capability: &str, calls: Vec<CallSink>, modules: &[&str]) -> SinkRule {
    SinkRule {
        capability: capability.to_owned(),
        calls,
        modules: modules.iter().map(|m| (*m).to_owned()).collect(),
    }
}

/// The builtin sink database, defined at the stdlib boundary.
pub fn builtin_sinks() -> Vec<SinkRule> {
    vec![
        rule(
            "time",
            vec![
                CallSink::exact("time.time"),
                CallSink::exact("time.time_ns"),
                CallSink::exact("time.monotonic"),
                CallSink::exact("time.monotonic_ns"),
                CallSink::exact("time.perf_counter"),
                CallSink::exact("time.perf_counter_ns"),
                CallSink::exact("time.process_time"),
                CallSink::exact("time.process_time_ns"),
                CallSink::exact("time.sleep"),
                // Deterministic when handed an explicit timestamp; ambient
                // clock reads only in the no-argument form.
                CallSink::no_args("time.localtime"),
                CallSink::no_args("time.gmtime"),
                CallSink::exact("datetime.datetime.now"),
                CallSink::exact("datetime.datetime.utcnow"),
                CallSink::exact("datetime.datetime.today"),
                CallSink::exact("datetime.date.today"),
                CallSink::exact("asyncio.sleep"),
            ],
            &[],
        ),
        rule(
            "random",
            vec![
                CallSink::exact("os.urandom"),
                CallSink::exact("os.getrandom"),
                CallSink::exact("uuid.uuid1"),
                CallSink::exact("uuid.uuid4"),
            ],
            &["random", "secrets", "numpy.random"],
        ),
        // hash_order has no named sinks: it is the syntactic set-construction
        // heuristic, applied in sim scope only (see `simscope`). The rule
        // exists so the capability can be granted by a shim declaration.
        rule("hash_order", vec![], &[]),
        rule(
            "thread",
            vec![CallSink::exact("os.fork")],
            &["threading", "multiprocessing", "concurrent.futures"],
        ),
        rule(
            "fs",
            vec![
                CallSink::exact("open"),
                CallSink::exact("io.open"),
                CallSink::exact("os.listdir"),
                CallSink::exact("os.scandir"),
                CallSink::exact("os.walk"),
                CallSink::exact("os.stat"),
                CallSink::exact("os.remove"),
                CallSink::exact("os.rename"),
                CallSink::exact("os.makedirs"),
            ],
            &["pathlib", "shutil", "tempfile", "glob"],
        ),
        rule(
            "net",
            vec![
                // Only asyncio's stream surface is fenced; a blanket `asyncio`
                // fence would swallow every sim harness that uses an event
                // loop, and `asyncio.sleep` is already a `time` sink.
                CallSink::exact("asyncio.open_connection"),
                CallSink::exact("asyncio.start_server"),
                CallSink::exact("asyncio.open_unix_connection"),
                CallSink::exact("asyncio.start_unix_server"),
            ],
            &["socket", "ssl", "http", "urllib", "asyncio.streams"],
        ),
        rule(
            "env",
            vec![
                CallSink::exact("os.environ"),
                CallSink::exact("os.getenv"),
                CallSink::exact("os.putenv"),
                CallSink::exact("sys.argv"),
            ],
            &["platform", "locale"],
        ),
        rule(
            "process",
            vec![
                CallSink::exact("os.system"),
                CallSink::wildcard("os.spawn"),
                CallSink::wildcard("os.exec"),
                CallSink::exact("os.getpid"),
                CallSink::exact("os.getppid"),
            ],
            &["subprocess", "signal"],
        ),
        rule(
            "io",
            vec![CallSink::exact("input"), CallSink::exact("sys.stdin")],
            &[],
        ),
    ]
}

/// Modules whose imports inside sim scope are silent even though no source for
/// them was collected. Trusting a module never masks a sink *inside* it: sinks
/// match on the name at the use site, not on the import.
///
/// The set is deliberately small — deterministic stdlib plumbing, plus the
/// modules that host call sinks (so `import time` is quiet while `time.time()`
/// is not).
pub fn builtin_trusted_modules() -> &'static [&'static str] {
    &[
        "__future__",
        "abc",
        "array",
        "base64",
        "binascii",
        "bisect",
        "collections",
        "contextlib",
        "copy",
        "dataclasses",
        "datetime",
        "decimal",
        "enum",
        "fractions",
        "functools",
        "hashlib",
        "heapq",
        "hmac",
        "inspect",
        "io",
        "itertools",
        "json",
        "logging",
        "math",
        "numbers",
        "operator",
        "os",
        "pprint",
        "queue",
        "re",
        "string",
        "struct",
        "sys",
        "textwrap",
        "time",
        "traceback",
        "trigpoint_shims",
        "types",
        "typing",
        "typing_extensions",
        "unicodedata",
        "uuid",
        "warnings",
        "weakref",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> SinkDb {
        SinkDb::new(builtin_sinks())
    }

    fn hit(db: &SinkDb, name: &str) -> Option<SinkHit> {
        db.match_name(name, Usage::Called { arguments: 0 })
    }

    #[test]
    fn exact_call_sinks_match() {
        let db = db();
        assert_eq!(
            hit(&db, "time.time").map(|h| h.capability).as_deref(),
            Some("time")
        );
        assert_eq!(
            hit(&db, "datetime.datetime.utcnow")
                .map(|h| h.capability)
                .as_deref(),
            Some("time")
        );
        assert!(hit(&db, "time.timezone").is_none());
    }

    #[test]
    fn module_fences_match_names_underneath() {
        let db = db();
        assert_eq!(
            hit(&db, "random.randint").map(|h| h.capability).as_deref(),
            Some("random")
        );
        assert_eq!(
            hit(&db, "random").map(|h| h.capability).as_deref(),
            Some("random")
        );
        // Fences respect dotted-segment boundaries.
        assert!(hit(&db, "randomizer.pick").is_none());
        assert_eq!(
            db.match_module("numpy.random.default_rng")
                .map(|h| h.capability)
                .as_deref(),
            Some("random")
        );
        assert!(db.match_module("numpy.linalg").is_none());
    }

    #[test]
    fn wildcard_call_sinks_match_families() {
        let db = db();
        assert_eq!(
            hit(&db, "os.spawnvpe").map(|h| h.capability).as_deref(),
            Some("process")
        );
        assert_eq!(
            hit(&db, "os.execv").map(|h| h.capability).as_deref(),
            Some("process")
        );
        assert!(hit(&db, "os.path").is_none());
    }

    #[test]
    fn arity_sensitive_sinks_respect_their_call_site() {
        let db = db();
        assert!(
            db.match_name("time.localtime", Usage::Called { arguments: 0 })
                .is_some()
        );
        assert!(
            db.match_name("time.localtime", Usage::Called { arguments: 1 })
                .is_none()
        );
        // A bare reference cannot be proven safe, so it stays a sink.
        assert!(db.match_name("time.localtime", Usage::Referenced).is_some());
    }

    #[test]
    fn precedence_is_exact_then_wildcard_then_fence() {
        let db = SinkDb::new(vec![
            rule("fence", vec![], &["pkg"]),
            rule("wild", vec![CallSink::wildcard("pkg.spawn")], &[]),
            rule("exact", vec![CallSink::exact("pkg.spawnl")], &[]),
        ]);
        assert_eq!(
            hit(&db, "pkg.spawnl").map(|h| h.capability).as_deref(),
            Some("exact")
        );
        assert_eq!(
            hit(&db, "pkg.spawnv").map(|h| h.capability).as_deref(),
            Some("wild")
        );
        assert_eq!(
            hit(&db, "pkg.other").map(|h| h.capability).as_deref(),
            Some("fence")
        );
    }

    #[test]
    fn print_is_not_a_sink() {
        let db = db();
        assert!(hit(&db, "print").is_none());
        assert!(hit(&db, "sys.stdout").is_none());
    }

    #[test]
    fn user_sinks_merge_with_builtins() {
        let spec = SinkSpec {
            capability: "time".to_owned(),
            calls: vec!["arrow.utcnow".to_owned()],
            modules: vec!["pendulum".to_owned()],
        };
        let mut rules = builtin_sinks();
        rules.push(SinkRule::from_spec(&spec));
        let db = SinkDb::new(rules);
        assert_eq!(
            hit(&db, "arrow.utcnow").map(|h| h.capability).as_deref(),
            Some("time")
        );
        assert_eq!(
            hit(&db, "pendulum.now").map(|h| h.capability).as_deref(),
            Some("time")
        );
        assert_eq!(
            hit(&db, "time.time").map(|h| h.capability).as_deref(),
            Some("time")
        );
    }
}
