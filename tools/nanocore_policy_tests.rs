//! Tests of the existing dependency planner and service manifest parser.
extern crate alloc;
#[path = "../src/nanocore/dag.rs"] mod dag;
#[path = "../src/nanocore/manifests.rs"] mod manifests;

#[test]
fn reads_every_builtin_service_and_dependency() {
    let services = manifests::builtin();
    assert_eq!(services.len(), 3);
    assert_eq!(services[0].name, "hb-monitor");
    assert_eq!(services[1].deps, ["hb-monitor"]);
    assert_eq!(services[2].deps, ["ipc-drain"]);
}

#[test]
fn rejects_incomplete_blocks_instead_of_discarding_them() {
    assert!(manifests::parse("[service]\nname=a\n[service]\nname=b\nspawn=cmd:b\n").is_none());
}

#[test]
fn detects_forward_cycles_and_preserves_graph() {
    let mut g = dag::Dag::new();
    assert!(!g.add("self", &["self"]));
    assert!(g.add("a", &["b"]));
    assert!(!g.add("b", &["a"]));
    assert_eq!(g.len(), 1);
    assert_eq!(g.missing_dependency(), Some(("a", "b")));
    assert!(g.add("b", &[]));
    assert!(g.missing_dependency().is_none());
    assert_eq!(g.execution_order().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["b", "a"]);
    assert!(!g.dependencies_ready("a"));
    g.stages_mut().iter_mut().find(|s| s.name == "b").unwrap().ok = true;
    assert!(g.dependencies_ready("a"));
    assert!(!g.dependencies_ready("missing"));
}

#[test]
fn rejects_longer_forward_cycles() {
    let mut g = dag::Dag::new();
    assert!(g.add("a", &["b"]));
    assert!(g.add("b", &["c"]));
    assert!(!g.add("c", &["a"]));
    assert_eq!(g.len(), 2);
}
