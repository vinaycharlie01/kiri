//! Architecture tests: the hexagonal dependency rules, checked on every
//! `cargo test` by reading each crate's manifest.
//!
//! - The domain depends on nothing.
//! - The application depends on the domain only.
//! - Adapters depend on the domain (the ports they implement), never on the
//!   application, the UI, another adapter or the Kiri binaries' internals.
//! - The UI depends on the application, never on an adapter.
//! - Only a composition root (`kai-desktop`) may depend on adapters.
//! - Upstream Kiri crates never depend on a Kai crate.
//! - No cycles between Kai crates.
//! - Every Kai crate is classified, so a new one cannot dodge the rules.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Manifest {
    name: String,
    /// `[dependencies]` and `[target.*.dependencies]`: what ships.
    normal: BTreeSet<String>,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Reads the package name and normal dependency names from a Cargo.toml.
/// Enough TOML for the manifests in this repository: section headers,
/// `name = …`, `name.workspace = true` and `name = { … }` lines.
fn read_manifest(path: &Path) -> Manifest {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut name = None;
    let mut normal = BTreeSet::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if section == "package" && key == "name" {
            name = Some(value.trim().trim_matches('"').to_owned());
        }
        let is_normal = section == "dependencies"
            || (section.starts_with("target.") && section.ends_with(".dependencies"));
        if is_normal {
            let dep = key
                .split('.')
                .next()
                .unwrap_or(key)
                .trim()
                .trim_matches('"');
            // `package = "…"` renames: the real crate name is what matters.
            let real = value
                .split("package")
                .nth(1)
                .and_then(|r| r.split('"').nth(1))
                .unwrap_or(dep);
            normal.insert(real.to_owned());
        }
    }
    Manifest {
        name: name.unwrap_or_else(|| panic!("{}: no package name", path.display())),
        normal,
    }
}

fn manifests_in(dir: &Path) -> Vec<Manifest> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("Cargo.toml"))
        .filter(|p| p.is_file())
        .map(|p| read_manifest(&p))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn all() -> (Vec<Manifest>, Vec<Manifest>) {
    let root = repo_root();
    let mut kai = Vec::new();
    let mut upstream = Vec::new();
    for m in manifests_in(&root.join("crates"))
        .into_iter()
        .chain(manifests_in(&root.join("desktop/crates")))
    {
        if m.name.starts_with("kai-") {
            kai.push(m)
        } else {
            upstream.push(m)
        }
    }
    (kai, upstream)
}

#[derive(Debug, PartialEq)]
enum Role {
    Domain,
    Application,
    Testkit,
    Adapter,
    Ui,
    CompositionRoot,
}

fn role(name: &str) -> Option<Role> {
    Some(match name {
        "kai-domain" => Role::Domain,
        "kai-application" => Role::Application,
        "kai-testkit" => Role::Testkit,
        "kai-ui" => Role::Ui,
        "kai-desktop" => Role::CompositionRoot,
        n if n.starts_with("kai-adapter-") => Role::Adapter,
        _ => return None,
    })
}

const UPSTREAM_BINARIES: [&str; 2] = ["kiri", "kiri-win"];

#[test]
fn the_crates_under_test_were_found() {
    let (kai, upstream) = all();
    let names: Vec<_> = kai.iter().map(|m| m.name.as_str()).collect();
    for expected in [
        "kai-domain",
        "kai-application",
        "kai-testkit",
        "kai-adapter-kiri",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} not found in {names:?}"
        );
    }
    assert!(
        upstream.iter().any(|m| m.name == "kiri"),
        "upstream kiri crate not found"
    );
}

#[test]
fn every_kai_crate_has_an_architectural_role() {
    let (kai, _) = all();
    for m in &kai {
        assert!(
            role(&m.name).is_some(),
            "{} has no role in tests/architecture.rs: classify it",
            m.name
        );
    }
}

#[test]
fn the_domain_depends_on_nothing() {
    let (kai, _) = all();
    let domain = kai.iter().find(|m| m.name == "kai-domain").unwrap();
    assert!(
        domain.normal.is_empty(),
        "kai-domain must depend on std only, has {:?}",
        domain.normal
    );
}

#[test]
fn the_application_depends_on_the_domain_only() {
    let (kai, _) = all();
    for m in kai
        .iter()
        .filter(|m| matches!(role(&m.name), Some(Role::Application | Role::Testkit)))
    {
        let extra: Vec<_> = m.normal.iter().filter(|d| *d != "kai-domain").collect();
        assert!(
            extra.is_empty(),
            "{} may depend on kai-domain only, also has {extra:?}",
            m.name
        );
    }
}

#[test]
fn adapters_implement_ports_and_reach_nothing_above_them() {
    let (kai, _) = all();
    for m in kai.iter().filter(|m| role(&m.name) == Some(Role::Adapter)) {
        assert!(
            m.normal.contains("kai-domain"),
            "{} implements no port: no kai-domain dependency",
            m.name
        );
        for d in &m.normal {
            let forbidden = matches!(
                role(d),
                Some(
                    Role::Application
                        | Role::Ui
                        | Role::CompositionRoot
                        | Role::Adapter
                        | Role::Testkit
                )
            ) || UPSTREAM_BINARIES.contains(&d.as_str());
            assert!(!forbidden, "adapter {} must not depend on {d}", m.name);
        }
    }
}

#[test]
fn the_ui_never_touches_infrastructure() {
    let (kai, _) = all();
    for m in kai.iter().filter(|m| role(&m.name) == Some(Role::Ui)) {
        assert!(
            m.normal.contains("kai-application"),
            "{} must drive the application layer",
            m.name
        );
        for d in &m.normal {
            let forbidden = matches!(
                role(d),
                Some(Role::Adapter | Role::Testkit | Role::CompositionRoot)
            ) || d.starts_with("kiri");
            assert!(!forbidden, "UI crate {} must not depend on {d}", m.name);
        }
    }
}

#[test]
fn upstream_kiri_never_depends_on_kai() {
    let (_, upstream) = all();
    for m in &upstream {
        let kai: Vec<_> = m.normal.iter().filter(|d| d.starts_with("kai-")).collect();
        assert!(
            kai.is_empty(),
            "upstream crate {} depends on {kai:?}",
            m.name
        );
    }
}

#[test]
fn kai_crates_have_no_dependency_cycles() {
    let (kai, _) = all();
    let graph: BTreeMap<&str, Vec<&str>> = kai
        .iter()
        .map(|m| {
            (
                m.name.as_str(),
                m.normal
                    .iter()
                    .map(String::as_str)
                    .filter(|d| d.starts_with("kai-"))
                    .collect(),
            )
        })
        .collect();
    // Depth-first search with colours: 1 = on the current path, 2 = done.
    fn visit<'a>(
        n: &'a str,
        g: &BTreeMap<&'a str, Vec<&'a str>>,
        colour: &mut BTreeMap<&'a str, u8>,
        path: &mut Vec<&'a str>,
    ) {
        match colour.get(n) {
            Some(1) => panic!("dependency cycle: {} -> {n}", path.join(" -> ")),
            Some(2) => return,
            _ => {}
        }
        colour.insert(n, 1);
        path.push(n);
        for d in g.get(n).into_iter().flatten() {
            visit(d, g, colour, path);
        }
        path.pop();
        colour.insert(n, 2);
    }
    let mut colour = BTreeMap::new();
    for n in graph.keys() {
        visit(n, &graph, &mut colour, &mut Vec::new());
    }
}

#[test]
fn the_manifest_reader_sees_renamed_and_target_specific_dependencies() {
    let dir = std::env::temp_dir().join(format!("kai-arch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Cargo.toml");
    std::fs::write(
        &path,
        "[package]\nname = \"x\"\n[dependencies]\na = \"1\"\nb.workspace = true\ngpui = { package = \"gpui-ce\", version = \"0.3\" } # c\n[dev-dependencies]\nd = \"1\"\n[target.'cfg(unix)'.dependencies]\ne = \"1\"\n",
    )
    .unwrap();
    let m = read_manifest(&path);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        m.normal,
        ["a", "b", "gpui-ce", "e"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}
