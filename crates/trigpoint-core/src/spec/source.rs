//! Resolve Rust test pointers from committed source without running build scripts
//! or tests. Ambiguous/cfg-generated/macro-generated functions remain unresolved.
use super::{PointerScheme, diff::Snapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use syn::spanned::Spanned;

pub type Files = BTreeMap<PathBuf, String>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSource {
    pub path: PathBuf,
    pub body: String,
    pub docs: String,
    pub docs_resolved: bool,
}
#[derive(Default)]
pub struct Index {
    tests: BTreeMap<String, Vec<TestSource>>,
}
impl Index {
    pub fn build(files: &Files) -> Self {
        let mut index = Self::default();
        for (path, text) in files
            .iter()
            .filter(|(p, _)| p.file_name().is_some_and(|n| n == "Cargo.toml"))
        {
            let Ok(manifest) = toml::from_str::<toml::Value>(text) else {
                continue;
            };
            let Some(package) = manifest.get("package") else {
                continue;
            };
            let Some(package_name) = package.get("name").and_then(toml::Value::as_str) else {
                continue;
            };
            let dir = path.parent().unwrap_or(Path::new(""));
            let mut targets = BTreeSet::new();
            let lib = manifest.get("lib");
            let lib_name = lib
                .and_then(|l| l.get("name"))
                .and_then(toml::Value::as_str)
                .unwrap_or(package_name);
            let lib_path = lib
                .and_then(|l| l.get("path"))
                .and_then(toml::Value::as_str)
                .unwrap_or("src/lib.rs");
            if lib.is_some() || package.get("autolib").and_then(toml::Value::as_bool) != Some(false)
            {
                targets.insert((lib_name.replace('-', "_"), normalize(&dir.join(lib_path))));
            }
            if package.get("autobins").and_then(toml::Value::as_bool) != Some(false) {
                targets.insert((package_name.replace('-', "_"), dir.join("src/main.rs")));
                auto_targets(files, &dir.join("src/bin"), &mut targets);
            }
            if package.get("autotests").and_then(toml::Value::as_bool) != Some(false) {
                auto_targets(files, &dir.join("tests"), &mut targets);
            }
            for kind in ["bin", "test"] {
                if let Some(entries) = manifest.get(kind).and_then(toml::Value::as_array) {
                    for target in entries {
                        if target.get("test").and_then(toml::Value::as_bool) == Some(false) {
                            continue;
                        }
                        if let Some(name) = target.get("name").and_then(toml::Value::as_str) {
                            let path = target
                                .get("path")
                                .and_then(toml::Value::as_str)
                                .map(|p| normalize(&dir.join(p)))
                                .unwrap_or_else(|| {
                                    dir.join(if kind == "test" {
                                        format!("tests/{name}.rs")
                                    } else {
                                        format!("src/bin/{name}.rs")
                                    })
                                });
                            targets.retain(|(n, _)| n != &name.replace('-', "_"));
                            targets.insert((name.replace('-', "_"), path));
                        }
                    }
                }
            }
            for (target, path) in targets {
                let module_dir = path.parent().unwrap_or(Path::new("")).to_owned();
                index.file(files, &path, &target, &module_dir, &mut BTreeSet::new());
            }
        }
        index
    }
    fn file(
        &mut self,
        files: &Files,
        path: &Path,
        prefix: &str,
        module_dir: &Path,
        stack: &mut BTreeSet<PathBuf>,
    ) {
        if !stack.insert(path.to_owned()) {
            return;
        }
        if let Some(text) = files.get(path) {
            if let Ok(parsed) = syn::parse_file(text) {
                self.items(files, &parsed.items, path, text, prefix, module_dir, stack);
            }
        }
        stack.remove(path);
    }
    #[allow(clippy::too_many_arguments)]
    fn items(
        &mut self,
        files: &Files,
        items: &[syn::Item],
        path: &Path,
        text: &str,
        prefix: &str,
        module_dir: &Path,
        stack: &mut BTreeSet<PathBuf>,
    ) {
        for item in items {
            match item {
                syn::Item::Fn(function) => {
                    let mut docs = Vec::new();
                    let mut docs_resolved = true;
                    let mut attrs = Vec::new();
                    for attr in &function.attrs {
                        if attr.path().is_ident("doc") {
                            match &attr.meta {
                                syn::Meta::NameValue(n) => match &n.value {
                                    syn::Expr::Lit(syn::ExprLit {
                                        lit: syn::Lit::Str(s),
                                        ..
                                    }) => docs.push(s.value()),
                                    _ => {
                                        docs_resolved = false;
                                        docs.push(slice(text, attr.span()));
                                    }
                                },
                                _ => {
                                    docs_resolved = false;
                                    docs.push(slice(text, attr.span()));
                                }
                            }
                        } else {
                            attrs.push(slice(text, attr.span()));
                        }
                    }
                    // The item span includes attributes; keep those (except docs)
                    // separately so a doc-only edit does not appear as a body edit.
                    let start = function.vis.span().start();
                    let start = if matches!(function.vis, syn::Visibility::Inherited) {
                        function.sig.span().start()
                    } else {
                        start
                    };
                    attrs.push(range(text, start, function.block.span().end()));
                    self.tests
                        .entry(format!("{prefix}::{}", function.sig.ident))
                        .or_default()
                        .push(TestSource {
                            path: path.to_owned(),
                            body: attrs.join("\n"),
                            docs: docs.join("\n"),
                            docs_resolved,
                        });
                }
                syn::Item::Mod(module) => {
                    let prefix = format!("{prefix}::{}", module.ident);
                    if let Some((_, items)) = &module.content {
                        self.items(
                            files,
                            items,
                            path,
                            text,
                            &prefix,
                            &module_dir.join(module.ident.to_string()),
                            stack,
                        );
                    } else {
                        // Conditional path attributes cannot be resolved without cfg;
                        // don't silently select a different source file.
                        if module.attrs.iter().any(|a| a.path().is_ident("cfg_attr")) {
                            continue;
                        }
                        let explicit = module.attrs.iter().find_map(|a| {
                            if !a.path().is_ident("path") {
                                return None;
                            }
                            match &a.meta {
                                syn::Meta::NameValue(n) => match &n.value {
                                    syn::Expr::Lit(syn::ExprLit {
                                        lit: syn::Lit::Str(s),
                                        ..
                                    }) => Some(s.value()),
                                    _ => None,
                                },
                                _ => None,
                            }
                        });
                        let candidates = if let Some(p) = explicit {
                            vec![normalize(&module_dir.join(p))]
                        } else {
                            vec![
                                module_dir.join(format!("{}.rs", module.ident)),
                                module_dir.join(module.ident.to_string()).join("mod.rs"),
                            ]
                        };
                        for child in candidates.into_iter().filter(|p| files.contains_key(p)) {
                            let child_dir = if child.file_name().is_some_and(|n| n == "mod.rs") {
                                child.parent().unwrap().to_owned()
                            } else {
                                child.with_extension("")
                            };
                            self.file(files, &child, &prefix, &child_dir, stack);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    pub fn resolve(&self, pointer: &str) -> Result<&TestSource, String> {
        match self.tests.get(pointer).map(Vec::as_slice) {
            Some([source]) => Ok(source),
            Some(_) => Err(format!("{pointer}: multiple possible definitions")),
            None => Err(format!(
                "{pointer}: no static Rust function found (missing, generated, or unsupported pointer)"
            )),
        }
    }
}

fn auto_targets(files: &Files, dir: &Path, targets: &mut BTreeSet<(String, PathBuf)>) {
    for path in files.keys() {
        if path.parent() == Some(dir) && path.extension().is_some_and(|e| e == "rs") {
            targets.insert((
                path.file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .replace('-', "_"),
                path.clone(),
            ));
        } else if path.file_name().is_some_and(|n| n == "main.rs")
            && path.parent().and_then(Path::parent) == Some(dir)
        {
            targets.insert((
                path.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .replace('-', "_"),
                path.clone(),
            ));
        }
    }
}
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            _ => out.push(c),
        }
    }
    out
}
fn slice(text: &str, span: proc_macro2::Span) -> String {
    range(text, span.start(), span.end())
}
fn range(text: &str, start: proc_macro2::LineColumn, end: proc_macro2::LineColumn) -> String {
    // proc-macro2 columns count Unicode scalar values, whereas string slices
    // use byte offsets. Translate within the line before slicing.
    let offset = |p: proc_macro2::LineColumn| {
        let line_start = text
            .split_inclusive('\n')
            .take(p.line.saturating_sub(1))
            .map(str::len)
            .sum::<usize>();
        let line = &text[line_start..];
        line_start
            + line
                .char_indices()
                .nth(p.column)
                .map(|(i, _)| i)
                .unwrap_or(line.len())
    };
    text.get(offset(start)..offset(end))
        .unwrap_or("")
        .to_owned()
}

pub fn enrich(snapshot: &mut Snapshot, index: &Index) {
    for entry in snapshot.values_mut() {
        for (kind, pointers) in entry.file.evidence.iter().flat_map(|e| &e.0) {
            if kind.pointer_scheme() != PointerScheme::Test {
                continue;
            }
            for pointer in pointers.iter() {
                match index.resolve(pointer) {
                    Ok(source) => {
                        if !source.docs_resolved {
                            entry.warnings.insert(format!("{}: {pointer}: dynamic doc attribute cannot be resolved statically", entry.file.invariant.id));
                        }
                        entry.facts.insert(format!(
                            "test body.{kind}.{pointer} = {}\n{}",
                            source.path.display(),
                            source.body
                        ));
                        entry
                            .facts
                            .insert(format!("test docstring.{kind}.{pointer} = {}", source.docs));
                    }
                    Err(error) => {
                        entry
                            .warnings
                            .insert(format!("{}: {error}", entry.file.invariant.id));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_modules_targets_and_separates_docs_from_body() {
        let files = Files::from([
            ("pkg/Cargo.toml".into(), "[package]\nname='demo-lib'".into()),
            ("pkg/src/lib.rs".into(), "#[cfg(test)] mod tests;".into()),
            (
                "pkg/src/tests.rs".into(),
                "/// old docs\n#[test]\nfn ticks() { assert!(true); }\nmod nested;".into(),
            ),
            ("pkg/src/tests/nested.rs".into(), "fn works() {}".into()),
            ("pkg/tests/api.rs".into(), "#[test] fn external() {}".into()),
        ]);
        let index = Index::build(&files);
        let test = index.resolve("demo_lib::tests::ticks").unwrap();
        assert_eq!(test.docs, " old docs");
        assert!(test.body.contains("assert!(true)"));
        assert!(!test.body.contains("old docs"));
        assert!(index.resolve("demo_lib::tests::nested::works").is_ok());
        assert!(index.resolve("api::external").is_ok());
        assert!(index.resolve("unknown::ticks").is_err());
    }
    #[test]
    fn custom_target_paths_unicode_and_dynamic_docs() {
        let files = Files::from([
            ("Cargo.toml".into(), "[package]\nname='demo'\n[lib]\nname='custom'\npath='code/root.rs'\n[[test]]\nname='integration'\npath='checks/custom.rs'".into()),
            ("code/root.rs".into(), "#[path=\"other.rs\"] mod tests;".into()),
            ("code/other.rs".into(), "/// café\nfn t() { let s = \"λ\"; assert_eq!(s, \"λ\"); }\n#[doc=include_str!(\"doc.md\")] fn dynamic() {}".into()),
            ("checks/custom.rs".into(), "fn works() {}".into()),
        ]);
        let index = Index::build(&files);
        let test = index.resolve("custom::tests::t").unwrap();
        assert!(test.body.ends_with(" }"), "{}", test.body);
        assert!(test.body.contains("λ"));
        assert!(index.resolve("integration::works").is_ok());
        assert!(
            !index
                .resolve("custom::tests::dynamic")
                .unwrap()
                .docs_resolved
        );
    }
    #[test]
    fn ambiguous_cfg_definitions_are_not_guessed() {
        let files = Files::from([
            ("Cargo.toml".into(), "[package]\nname='demo'".into()),
            (
                "src/lib.rs".into(),
                "#[cfg(a)] fn t() {} #[cfg(b)] fn t() {}".into(),
            ),
        ]);
        assert!(
            Index::build(&files)
                .resolve("demo::t")
                .unwrap_err()
                .contains("multiple")
        );
    }
}
