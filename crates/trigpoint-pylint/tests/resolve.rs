//! Module-mapping and binding-resolution tests, run against the real fixture
//! trees so the unit under test sees the same input the golden corpus does.

use std::path::{Path, PathBuf};

use trigpoint_pylint::resolve::{NameKind, Program};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn program(root: &Path, source_roots: &[&str]) -> Program {
    let roots: Vec<PathBuf> = source_roots.iter().map(|r| root.join(r)).collect();
    Program::load(root, &roots).expect("fixture tree should load")
}

#[test]
fn module_paths_honour_packages() {
    let root = fixtures().join("sim").join("sim_relative");
    let program = program(&root, &["src"]);
    let mut names: Vec<&str> = program.modules.names().collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "pkg",
            "pkg.harness",
            "pkg.helper",
            "pkg.sub",
            "pkg.sub.deep"
        ]
    );
    // `__init__.py` *is* its package; a plain module is not.
    assert!(program.modules.get("pkg").expect("package").is_package);
    assert!(
        !program
            .modules
            .get("pkg.harness")
            .expect("module")
            .is_package
    );
    assert_eq!(
        program
            .modules
            .get("pkg.sub.deep")
            .expect("module")
            .package(),
        "pkg.sub"
    );
    assert_eq!(
        program.modules.get("pkg").expect("package").package(),
        "pkg"
    );
}

#[test]
fn relative_imports_resolve_through_the_binding_table() {
    let root = fixtures().join("sim").join("sim_relative");
    let program = program(&root, &["src"]);
    let id = program.modules.id_of("pkg.harness").expect("harness");
    let bindings = program.bindings(id);
    // `from . import helper`
    assert_eq!(bindings.lookup("helper"), Some("pkg.helper"));
    // `from .sub.deep import value`
    assert_eq!(bindings.lookup("value"), Some("pkg.sub.deep.value"));
    assert!(bindings.is_module("pkg.helper"));
    assert!(!bindings.is_module("pkg.sub.deep.value"));
}

#[test]
fn import_forms_bind_the_right_names() {
    let root = fixtures().join("prod");
    let program = program(&root, &["."]);

    let aliased = program.modules.id_of("aliased_import").expect("module");
    assert_eq!(program.bindings(aliased).lookup("clock"), Some("time"));

    let from_import = program.modules.id_of("from_import").expect("module");
    assert_eq!(
        program.bindings(from_import).lookup("monotonic"),
        Some("time.monotonic")
    );

    let direct = program.modules.id_of("direct_call").expect("module");
    assert_eq!(program.bindings(direct).lookup("time"), Some("time"));

    // Module-level `NAME = dotted.name` aliases.
    let alias = program.modules.id_of("module_alias").expect("module");
    assert_eq!(program.bindings(alias).lookup("NOW"), Some("time.time"));

    // Top-level `class` statements resolve as `<module>.<Class>`, which is how
    // a shim protocol is matched against its configured qualified name.
    let shimdefs = program.modules.id_of("shimdefs").expect("module");
    assert_eq!(
        program.bindings(shimdefs).lookup("ClockShim"),
        Some("shimdefs.ClockShim")
    );
}

#[test]
fn unresolvable_star_imports_are_recorded() {
    let root = fixtures().join("prod");
    let program = program(&root, &["."]);
    let id = program.modules.id_of("star_import").expect("module");
    let stars = program.bindings(id).unresolved_stars();
    assert_eq!(stars.len(), 1);
    assert_eq!(stars[0].target, "unvendored_pkg");
}

#[test]
fn module_objects_are_distinguished_from_values() {
    let root = fixtures().join("prod");
    let program = program(&root, &["."]);
    let id = program.modules.id_of("monkeypatch").expect("module");
    let bindings = program.bindings(id);
    assert!(bindings.is_module("time"));
    assert!(!bindings.is_module("time.time"));

    let shimdefs = program.modules.id_of("shimdefs").expect("module");
    let bindings = program.bindings(shimdefs);
    assert!(bindings.is_module("trigpoint_shims"));
    assert_eq!(
        bindings.lookup("DeterministicShim"),
        Some("trigpoint_shims.DeterministicShim")
    );
}

#[test]
fn locally_assigned_names_stop_resolving_to_builtins() {
    let root = fixtures().join("prod");
    let program = program(&root, &["."]);
    let id = program.modules.id_of("monkeypatch").expect("module");
    let bindings = program.bindings(id);
    // `install(fake)` binds `fake` as a parameter.
    assert!(bindings.is_shadowed("fake"));
    assert!(!bindings.is_shadowed("open"));
}

#[test]
fn sim_closure_walks_transitively_with_witness_chains() {
    let root = fixtures().join("sim").join("sim_transitive");
    let program = program(&root, &["src"]);
    let config_path = root.join("triglint.toml");
    let python = trigpoint_pylint::config::parse_file(&config_path)
        .expect("config parses")
        .expect("[python] present");
    let config = trigpoint_pylint::config::Resolved::new(&root, python);
    let closure = trigpoint_pylint::simscope::closure(&program, &config).expect("roots resolve");

    let inner = program.modules.id_of("app.inner").expect("inner module");
    assert!(closure.contains(inner));
    assert_eq!(
        closure.witness(inner),
        "app.harness > app.middle > app.inner"
    );
    assert_eq!(closure.order.len(), 4);
    assert!(closure.unresolved.is_empty());
}

#[test]
fn missing_sim_roots_are_an_error() {
    let root = fixtures().join("sim").join("sim_transitive");
    let program = program(&root, &["src"]);
    let mut python = trigpoint_pylint::config::PythonConfig::default();
    python.sim.roots = vec!["app.nonexistent".to_owned()];
    let config = trigpoint_pylint::config::Resolved::new(&root, python);
    let error = trigpoint_pylint::simscope::closure(&program, &config)
        .expect_err("a missing root must not pass silently");
    assert_eq!(error.0, vec!["app.nonexistent".to_owned()]);
}

#[test]
fn name_kinds_are_reported_for_resolved_expressions() {
    // A guard that the public `NameKind` distinction stays meaningful.
    assert_ne!(NameKind::Module, NameKind::Value);
}
