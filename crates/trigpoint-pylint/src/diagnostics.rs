//! Diagnostic model, `annotate-snippets` rendering, and the allow-comment
//! escape hatch.
//!
//! Two renderings exist and both are part of the contract:
//!
//! * [`Diagnostic::summary`] — one stable line per finding, the form the
//!   fixture golden files are written in.
//! * [`Diagnostic::render`] — the rustc-style snippet a human reads.
//!
//! The escape hatch is `# triglint: allow(shim-nondeterminism)`, honoured on
//! the finding's own line, on a standalone comment line directly above it, or
//! on the `def`/`class` header line of an enclosing definition — the Python
//! analogue of `#[allow(...)]` at an item.

use std::path::PathBuf;

use annotate_snippets::{AnnotationKind, Level as SnippetLevel, Renderer, Snippet};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Expr, ModModule, Stmt};
use ruff_text_size::{Ranged, TextRange, TextSize};

/// Whether a finding fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Fails the run.
    Deny,
    /// Reported, does not fail the run.
    Warn,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Deny => "deny",
            Level::Warn => "warn",
        }
    }
}

/// The lints this crate emits. The level is a property of the lint, so a
/// deny-level finding cannot be constructed as a warning by mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lint {
    /// Prod mode: a sink named outside a covering shim implementation.
    ShimNondeterminism,
    /// Sim mode: a sink named anywhere in the import closure of a sim root.
    SimNondeterminism,
    /// Something the analysis cannot see through.
    Unresolved,
}

impl Lint {
    pub fn name(self) -> &'static str {
        match self {
            Lint::ShimNondeterminism => "shim-nondeterminism",
            Lint::SimNondeterminism => "sim-nondeterminism",
            Lint::Unresolved => "unresolved",
        }
    }

    pub fn level(self) -> Level {
        match self {
            Lint::ShimNondeterminism | Lint::SimNondeterminism => Level::Deny,
            Lint::Unresolved => Level::Warn,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub lint: Lint,
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Path as shown to the user, relative to the config directory.
    pub display_path: String,
    pub range: TextRange,
    /// 1-based line of `range.start()`.
    pub line: u32,
    /// 1-based character column of `range.start()`.
    pub column: u32,
    /// Headline of the rendered report.
    pub title: String,
    /// Inline label under the highlighted span.
    pub label: String,
    /// Trailing notes: witness chains, blessing context, fix guidance.
    pub notes: Vec<String>,
    /// Stable machine-comparable tail, used by the golden fixtures.
    pub detail: String,
}

impl Diagnostic {
    pub fn level(&self) -> Level {
        self.lint.level()
    }

    /// One line, stable across renderer versions: the golden-file format.
    pub fn summary(&self) -> String {
        format!(
            "{} {} {}:{}:{} {}",
            self.level().as_str(),
            self.lint.name(),
            self.display_path,
            self.line,
            self.column,
            self.detail
        )
    }

    /// The human-facing snippet. `source` must be the file this diagnostic
    /// points into.
    pub fn render(&self, source: &str) -> String {
        let level = match self.level() {
            Level::Deny => SnippetLevel::ERROR,
            Level::Warn => SnippetLevel::WARNING,
        };
        let span = usize::from(self.range.start())..usize::from(self.range.end());
        let mut group = level
            .primary_title(self.title.as_str())
            .id(self.lint.name())
            .element(
                Snippet::source(source)
                    .path(self.display_path.as_str())
                    .fold(true)
                    .annotation(
                        AnnotationKind::Primary
                            .span(span)
                            .label(self.label.as_str()),
                    ),
            );
        for note in &self.notes {
            group = group.element(SnippetLevel::NOTE.message(note.as_str()));
        }
        Renderer::plain().render(&[group])
    }
}

/// Byte offset → (line, column), both 1-based.
#[derive(Debug)]
pub struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let mut starts = vec![0u32];
        for (offset, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                let next = u32::try_from(offset + 1).unwrap_or(u32::MAX);
                starts.push(next);
            }
        }
        Self { starts }
    }

    pub fn line(&self, offset: TextSize) -> u32 {
        let offset = u32::from(offset);
        match self.starts.binary_search(&offset) {
            Ok(index) => u32::try_from(index).unwrap_or(0) + 1,
            Err(index) => u32::try_from(index).unwrap_or(1),
        }
    }

    pub fn line_column(&self, source: &str, offset: TextSize) -> (u32, u32) {
        let line = self.line(offset);
        let start = self.starts[(line - 1) as usize] as usize;
        let end = usize::from(offset).min(source.len());
        let column = source
            .get(start..end)
            .map_or(0, |text| text.chars().count());
        (line, u32::try_from(column).unwrap_or(0) + 1)
    }
}

/// The `# triglint: allow(...)` markers found in one file.
#[derive(Debug, Default)]
pub struct AllowComments {
    /// Line number → lint names allowed on that line.
    by_line: std::collections::BTreeMap<u32, Vec<String>>,
    /// Definition ranges whose header line carried a marker.
    by_range: Vec<(TextRange, Vec<String>)>,
}

impl AllowComments {
    pub fn scan(source: &str, module: &ModModule, index: &LineIndex) -> Self {
        let mut by_line: std::collections::BTreeMap<u32, Vec<String>> =
            std::collections::BTreeMap::new();
        let mut standalone: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for (number, text) in source.lines().enumerate() {
            let line = u32::try_from(number).unwrap_or(0) + 1;
            let Some(names) = parse_allow(text) else {
                continue;
            };
            if text.trim_start().starts_with('#') {
                standalone.insert(line);
            }
            by_line.insert(line, names);
        }
        // A standalone marker also covers the line beneath it.
        for line in standalone {
            if let Some(names) = by_line.get(&line).cloned() {
                by_line.entry(line + 1).or_insert(names);
            }
        }

        let mut collector = DefCollector {
            index,
            by_line: &by_line,
            by_range: Vec::new(),
        };
        collector.visit_body(&module.body);
        Self {
            by_range: collector.by_range,
            by_line,
        }
    }

    pub fn suppresses(&self, lint: Lint, line: u32, offset: TextSize) -> bool {
        if let Some(names) = self.by_line.get(&line)
            && names.iter().any(|n| n == lint.name())
        {
            return true;
        }
        self.by_range
            .iter()
            .any(|(range, names)| range.contains(offset) && names.iter().any(|n| n == lint.name()))
    }
}

struct DefCollector<'a> {
    index: &'a LineIndex,
    by_line: &'a std::collections::BTreeMap<u32, Vec<String>>,
    by_range: Vec<(TextRange, Vec<String>)>,
}

impl<'a> Visitor<'a> for DefCollector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        let header = match stmt {
            Stmt::FunctionDef(function) => Some(function.name.range().start()),
            Stmt::ClassDef(class) => Some(class.name.range().start()),
            _ => None,
        };
        if let Some(header) = header {
            let line = self.index.line(header);
            if let Some(names) = self.by_line.get(&line) {
                self.by_range.push((stmt.range(), names.clone()));
            }
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, _expr: &'a Expr) {}
}

/// Parses `# triglint: allow(a, b)` out of a source line, if present.
fn parse_allow(text: &str) -> Option<Vec<String>> {
    let comment = text.find('#')?;
    let body = text[comment + 1..].trim();
    let rest = body.strip_prefix("triglint:")?.trim_start();
    let rest = rest.strip_prefix("allow(")?;
    let end = rest.find(')')?;
    let names: Vec<String> = rest[..end]
        .split(',')
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() { None } else { Some(names) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_lint() {
        assert_eq!(Lint::ShimNondeterminism.level(), Level::Deny);
        assert_eq!(Lint::SimNondeterminism.level(), Level::Deny);
        assert_eq!(Lint::Unresolved.level(), Level::Warn);
    }

    #[test]
    fn allow_comments_parse() {
        assert_eq!(
            parse_allow("x = time.time()  # triglint: allow(shim-nondeterminism)"),
            Some(vec!["shim-nondeterminism".to_owned()])
        );
        assert_eq!(
            parse_allow("# triglint: allow(shim-nondeterminism, unresolved)"),
            Some(vec![
                "shim-nondeterminism".to_owned(),
                "unresolved".to_owned()
            ])
        );
        assert_eq!(parse_allow("# just a comment"), None);
        assert_eq!(parse_allow("x = 1"), None);
        assert_eq!(parse_allow("# triglint: allow()"), None);
    }

    #[test]
    fn line_index_counts_from_one() {
        let source = "a = 1\nb = 2\n";
        let index = LineIndex::new(source);
        assert_eq!(index.line_column(source, TextSize::from(0)), (1, 1));
        assert_eq!(index.line_column(source, TextSize::from(6)), (2, 1));
        assert_eq!(index.line_column(source, TextSize::from(10)), (2, 5));
    }
}
