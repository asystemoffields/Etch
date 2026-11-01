//! Dependency layering rules (docs/architecture.md).
//!
//! The rule engine works on a small, metadata-independent model so it can be
//! unit-tested; `from_metadata` builds that model from `cargo metadata`.

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Layer 0, and additionally may depend on no workspace crate at all.
    Standalone,
    /// Test tooling: may depend on anything up to L5 (it sits at L6 for rule
    /// purposes); other crates may use it only as a dev-dependency.
    Testkit,
    /// Binaries and build tooling: exempt from the rules.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Standalone => Some(0),
            Class::Testkit => Some(6),
            Class::Exempt => None,
        }
    }
}

/// The layering table. Names are package names without the `wordcraft-`
/// prefix.
pub const TABLE: &[(&str, Class)] = &[
    ("geom", Class::Layer(0)),
    ("control-key", Class::Standalone),
    ("tz", Class::Standalone),
    ("doc", Class::Layer(1)),
    ("fonts", Class::Layer(1)),
    ("metafile", Class::Layer(1)),
    ("proof", Class::Layer(1)),
    ("layout", Class::Layer(2)),
    ("docx", Class::Layer(2)),
    ("docbin", Class::Layer(2)),
    ("formats", Class::Layer(2)),
    ("render", Class::Layer(3)),
    ("pdf", Class::Layer(3)),
    ("engine", Class::Layer(4)),
    ("zotero", Class::Layer(5)),
    ("ui-egui", Class::Layer(6)),
    ("testkit", Class::Testkit),
    // apps and tooling
    ("wordcraft", Class::Exempt),
    ("cli", Class::Exempt),
    ("web", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// Explicit orderings *within* a layer (earlier may be used by later).
/// The L0 foundation is a small chain: `raster` builds on `color` and
/// `geom`, which the §3 diagram draws on one line. The GPU backend (`gpu`)
/// reuses the CPU reference (`compose`) for LUTs and parity tests.
pub const INTRA_LAYER_ORDER: &[&[&str]] = &[&["doc", "fonts"]];

fn intra_layer_allowed(from: &str, to: &str) -> bool {
    let (from, to) = (short_name(from), short_name(to));
    INTRA_LAYER_ORDER.iter().any(|chain| match (chain.iter().position(|n| *n == from), chain.iter().position(|n| *n == to)) {
        (Some(f), Some(t)) => t < f,
        _ => false,
    })
}

/// External crates that constitute a UI toolkit / windowing dependency.
/// Entries ending in `*` are prefixes (`egui*` covers `egui_extras`, `egui_kittest`, `egui-winit`…).
pub const UI_CRATES: &[&str] = &["egui*", "eframe*", "winit*", "rfd*", "bevy*"];

/// First layer allowed to use UI crates.
pub const UI_MIN_LAYER: u8 = 6;

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("wordcraft-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// `true` if the dependency is a workspace member.
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
    /// External packages reached through normal and build dependencies, at any depth (from the
    /// resolved graph): a UI toolkit pulled in indirectly counts too.
    pub reach: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, dep: String, from: u8, to: u8, kind: DepKind },
    StandaloneHasWorkspaceDep { krate: String, dep: String },
    TestkitAsNormalDep { krate: String },
    UiBelowL6 { krate: String, dep: String, layer: u8 },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate}: unknown workspace crate; register it in xtask/src/layers.rs TABLE (see docs/architecture.md)")
            }
            Violation::Upward { krate, dep, from, to, kind } => {
                write!(f, "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on strictly lower layers")
            }
            Violation::StandaloneHasWorkspaceDep { krate, dep } => {
                write!(f, "{krate}: standalone crate must not depend on workspace crate {dep}")
            }
            Violation::TestkitAsNormalDep { krate } => {
                write!(f, "{krate}: wordcraft-testkit may only be a dev-dependency")
            }
            Violation::UiBelowL6 { krate, dep, layer } => {
                write!(f, "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L6+")
            }
        }
    }
}

/// Check all rules. Returns violations sorted for stable output.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let Some(class) = classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        if class == Class::Exempt {
            continue;
        }
        let layer = class.layer().unwrap_or(0);
        for d in &c.deps {
            // Self dev-dependencies (e.g. to enable features in tests) are fine.
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                if class == Class::Standalone {
                    out.push(Violation::StandaloneHasWorkspaceDep { krate: c.name.clone(), dep: d.name.clone() });
                    continue;
                }
                match classify(&d.name) {
                    Some(Class::Testkit) if d.kind != DepKind::Dev => {
                        out.push(Violation::TestkitAsNormalDep { krate: c.name.clone() });
                    }
                    // Unregistered deps are reported on their own entry; testkit dev deps are allowed.
                    None | Some(Class::Testkit) => {}
                    Some(dc) => {
                        let to = dc.layer().unwrap_or(u8::MAX);
                        if to >= layer && !(to == layer && intra_layer_allowed(&c.name, &d.name)) {
                            out.push(Violation::Upward {
                                krate: c.name.clone(),
                                dep: d.name.clone(),
                                from: layer,
                                to: if to == u8::MAX { 7 } else { to },
                                kind: d.kind,
                            });
                        }
                    }
                }
            } else if layer < UI_MIN_LAYER && is_ui_crate(&d.name) {
                out.push(Violation::UiBelowL6 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
        if layer < UI_MIN_LAYER {
            for r in c.reach.iter().filter(|r| is_ui_crate(r)) {
                out.push(Violation::UiBelowL6 { krate: c.name.clone(), dep: r.clone(), layer });
            }
        }
    }
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

/// Build the model from `cargo metadata --format-version 1` (with the resolved graph; without it,
/// as from `--no-deps`, `reach` stays empty and only direct dependencies are checked).
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let member_ids: Vec<&str> = meta["workspace_members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let is_member = |p: &Value| member_ids.is_empty() || p["id"].as_str().is_some_and(|id| member_ids.contains(&id));
    let members: Vec<&str> = pkgs.iter().filter(|p| is_member(p)).filter_map(|p| p["name"].as_str()).collect();
    let names: std::collections::HashMap<&str, &str> = pkgs.iter().filter_map(|p| Some((p["id"].as_str()?, p["name"].as_str()?))).collect();
    // id -> ids reached by normal or build edges.
    let mut edges: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for node in meta["resolve"]["nodes"].as_array().into_iter().flatten() {
        let Some(id) = node["id"].as_str() else { continue };
        let to = node["deps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"].as_str() != Some("dev")))
            .filter_map(|d| d["pkg"].as_str())
            .collect();
        edges.insert(id, to);
    }
    let mut out = Vec::new();
    for p in pkgs.iter().filter(|p| is_member(p)) {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let workspace = members.contains(&dname.as_str()) || d["path"].is_string();
            deps.push(Dep { name: dname, kind, workspace });
        }
        let mut reach = std::collections::BTreeSet::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack: Vec<&str> = p["id"].as_str().and_then(|id| edges.get(id)).cloned().unwrap_or_default();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(n) = names.get(id).filter(|n| !members.contains(n)) {
                reach.insert((*n).to_owned());
            }
            stack.extend(edges.get(id).into_iter().flatten());
        }
        out.push(Crate { name, deps, reach: reach.into_iter().collect() });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Standalone) => "L0 standalone".into(),
        Some(Class::Testkit) => "testkit".into(),
        Some(Class::Exempt) => "exempt".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate {
            name: name.into(),
            deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect(),
            reach: Vec::new(),
        }
    }
    use DepKind::*;

    #[test]
    fn clean_downward_graph_passes() {
        let g = [
            c("wordcraft-geom", &[("kurbo", Normal, false)]),
            c("wordcraft-doc", &[("wordcraft-geom", Normal, true)]),
            c("wordcraft-engine", &[("wordcraft-doc", Normal, true), ("wordcraft-testkit", Dev, true)]),
            c("wordcraft-ui-egui", &[("wordcraft-engine", Normal, true), ("egui", Normal, false)]),
            c("wordcraft-cli", &[("wordcraft-ui-egui", Normal, true)]),
        ];
        assert!(check(&g).is_empty(), "{:?}", check(&g));
    }

    #[test]
    fn upward_dependency_flagged() {
        let v = check(&[c("wordcraft-doc", &[("wordcraft-engine", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 1, to: 4, .. }]));
    }

    #[test]
    fn sideways_dependency_flagged() {
        let v = check(&[c("wordcraft-layout", &[("wordcraft-docx", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 2, to: 2, .. }]));
    }

    #[test]
    fn l0_foundation_chain_allowed_one_way() {
        assert!(check(&[c("wordcraft-doc", &[("wordcraft-geom", Normal, true)])]).is_empty());
        let v = check(&[c("wordcraft-geom", &[("wordcraft-doc", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 0, to: 1, .. }]));
    }

    #[test]
    fn self_dev_dependency_ignored() {
        assert!(check(&[c("wordcraft-doc", &[("wordcraft-doc", Dev, true)])]).is_empty());
    }

    #[test]
    fn upward_dev_dependency_flagged() {
        let v = check(&[c("wordcraft-geom", &[("wordcraft-doc", Dev, true)])]);
        assert!(matches!(v[..], [Violation::Upward { kind: Dev, .. }]));
    }

    #[test]
    fn ui_crates_below_l6_flagged() {
        for dep in ["egui", "eframe", "winit", "egui_kittest", "egui_extras", "egui-winit", "rfd", "bevy_ecs", "bevy"] {
            let v = check(&[c("wordcraft-engine", &[(dep, Normal, false)])]);
            assert!(matches!(v[..], [Violation::UiBelowL6 { layer: 4, .. }]), "{dep}");
        }
        assert!(check(&[c("wordcraft-engine", &[("serde", Normal, false), ("kurbo", Normal, false)])]).is_empty());
        assert!(!check(&[c("wordcraft-zotero", &[("winit", Normal, false)])]).is_empty());
    }

    #[test]
    fn ui_crates_reached_indirectly_are_flagged() {
        let mut k = c("wordcraft-render", &[("some-plotting", Normal, false)]);
        k.reach = vec!["some-plotting".into(), "winit".into()];
        let v = check(&[k.clone()]);
        assert!(matches!(&v[..], [Violation::UiBelowL6 { dep, layer: 3, .. }] if dep == "winit"), "{v:?}");
        let mut ui = k;
        ui.name = "wordcraft-ui-egui".into();
        assert!(check(&[ui]).is_empty());
    }

    #[test]
    fn resolved_graph_is_walked_without_dev_edges() {
        let meta: Value = serde_json::from_str(
            r#"{"workspace_members":["r"],
                "packages":[
                  {"id":"r","name":"wordcraft-render","dependencies":[{"name":"plot","kind":null},{"name":"egui_kittest","kind":"dev"}]},
                  {"id":"p","name":"plot","dependencies":[]},
                  {"id":"w","name":"winit","dependencies":[]},
                  {"id":"k","name":"egui_kittest","dependencies":[]}],
                "resolve":{"nodes":[
                  {"id":"r","deps":[{"pkg":"p","dep_kinds":[{"kind":null}]},{"pkg":"k","dep_kinds":[{"kind":"dev"}]}]},
                  {"id":"p","deps":[{"pkg":"w","dep_kinds":[{"kind":null}]}]},
                  {"id":"w","deps":[]},{"id":"k","deps":[]}]}}"#,
        )
        .unwrap();
        let g = from_metadata(&meta).unwrap();
        assert_eq!(g.len(), 1, "only workspace members");
        assert_eq!(g[0].reach, vec!["plot".to_string(), "winit".into()]);
        let v = check(&g);
        // winit through `plot`, and the direct dev dependency on egui_kittest.
        assert_eq!(v.len(), 2, "{v:?}");
    }

    #[test]
    fn unregistered_crate_is_error() {
        let v = check(&[c("wordcraft-mystery", &[])]);
        assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "wordcraft-mystery"));
        assert!(v[0].to_string().contains("register"));
    }

    #[test]
    fn testkit_only_as_dev_dependency() {
        let v = check(&[c("wordcraft-render", &[("wordcraft-testkit", Normal, true)])]);
        assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]));
        assert!(check(&[c("wordcraft-render", &[("wordcraft-testkit", Dev, true)])]).is_empty());
        // testkit itself may use anything up to L5 but not L6 crates.
        assert!(check(&[c("wordcraft-testkit", &[("wordcraft-engine", Normal, true)])]).is_empty());
        assert!(!check(&[c("wordcraft-testkit", &[("wordcraft-ui-egui", Normal, true)])]).is_empty());
    }

    #[test]
    fn apps_and_xtask_exempt() {
        for app in ["wordcraft", "wordcraft-cli", "wordcraft-web", "xtask"] {
            assert!(check(&[c(app, &[("egui", Normal, false), ("wordcraft-ui-egui", Normal, true)])]).is_empty());
        }
    }

    #[test]
    fn metadata_parsing() {
        let meta: Value = serde_json::from_str(
            r#"{"packages":[
                {"name":"wordcraft-doc","dependencies":[
                    {"name":"wordcraft-geom","kind":null,"path":"/x/crates/geom"},
                    {"name":"serde","kind":null},
                    {"name":"proptest","kind":"dev"}]},
                {"name":"wordcraft-geom","dependencies":[]}
            ]}"#,
        )
        .unwrap();
        let g = from_metadata(&meta).unwrap();
        assert_eq!(g.len(), 2);
        let doc = g.iter().find(|c| c.name == "wordcraft-doc").unwrap();
        assert!(doc.deps[0].workspace && !doc.deps[1].workspace);
        assert_eq!(doc.deps[2].kind, Dev);
        assert!(check(&g).is_empty());
    }
}
