use anyhow::{anyhow, Context, Result};
use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::TurboJson;
use crate::sidecar::Sidecar;

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    /// Short alias (directory name, or unscoped tail of name)
    pub short: String,
    pub path: PathBuf,
    pub scripts: BTreeMap<String, String>,
    pub deps: Vec<String>,
    /// Per-package `turbo.json` (sibling to `package.json`). Overrides root for
    /// task entries that exist here; absent entries fall through to the root.
    pub turbo: Option<TurboJson>,
    /// True when this is the project's root package (lives at `Project::root`).
    /// Root scripts tend to be aggregators (`turbo run dev -F …`) that would
    /// nest turbo inside procpane, so bare-name task expansion skips them by
    /// convention — mirrors turbo's own rule that root tasks must be
    /// addressed as `//#task` in turbo.json.
    pub is_root: bool,
}

/// A registered repository: the thing that gets a tab, and the thing whose
/// worktrees become workspaces. Named `Workspace` in procpane, where it meant
/// "the turborepo root"; that name now belongs to a worktree and its
/// environment, so the repo-level concept is a `Project`.
///
/// Note the two senses of "workspace" in this file: ours is a git worktree,
/// while `pnpm-workspace.yaml` and `package.json#workspaces` are the JS
/// ecosystem's term for package globs. The latter keep their names.
#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub turbo: TurboJson,
    pub sidecar: Sidecar,
    pub packages: Vec<Package>,
    /// Detected package manager command: "pnpm" | "npm" | "yarn" | "bun"
    pub pkg_manager: String,
}

#[derive(Deserialize)]
struct RootPackageJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    workspaces: Option<WorkspacesField>,
    #[serde(default, rename = "packageManager")]
    package_manager: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WorkspacesField {
    Plain(Vec<String>),
    Object {
        #[serde(default)]
        packages: Vec<String>,
    },
}

#[derive(Deserialize)]
struct PnpmWorkspace {
    #[serde(default)]
    packages: Vec<String>,
}

#[derive(Deserialize)]
struct PackageJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    scripts: BTreeMap<String, String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies")]
    dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    peer_dependencies: BTreeMap<String, String>,
}

impl Project {
    pub fn discover(start: &Path) -> Result<Self> {
        let root = find_root(start)?;
        let turbo_path = root.join("turbo.json");
        // A project may have no turbo at all — `preceipts.toml` alone is a
        // complete description of one. An empty TurboJson contributes no
        // tasks, which is exactly right: everything then comes from the
        // manifest.
        let turbo = if turbo_path.is_file() {
            TurboJson::load(&turbo_path)?
        } else {
            TurboJson::default()
        };
        let sidecar = Sidecar::load(&root)?;

        let patterns = discover_workspace_patterns(&root)?;
        let pkg_manager = detect_package_manager(&root);
        let mut packages = Vec::new();

        // Root package is its own "package" for root-level tasks. Mark it
        // explicitly so bare-name task expansion can skip it (its scripts
        // are usually aggregators that nest turbo inside procpane).
        if let Ok(mut root_pkg) = read_package(&root) {
            root_pkg.is_root = true;
            packages.push(root_pkg);
        }

        // Services declared with `run` are not turbo tasks, but the daemon
        // addresses everything as `pkg#task`. They become scripts on a
        // synthetic package so the graph builder finds them the ordinary way,
        // rather than growing a second code path for a second kind of node.
        if let Some(synthetic) = synthetic_package(&root)? {
            packages.push(synthetic);
        }

        let mut builder = GlobSetBuilder::new();
        for p in &patterns {
            // Workspace globs match dirs; ensure trailing /package.json
            let g = Glob::new(p).with_context(|| format!("invalid workspace glob: {p}"))?;
            builder.add(g);
        }
        let globset = builder.build()?;

        for entry in walkdir::WalkDir::new(&root)
            .min_depth(1)
            .max_depth(6)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                name != "node_modules" && !name.starts_with('.')
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_dir() {
                continue;
            }
            let rel = match entry.path().strip_prefix(&root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if !globset.is_match(rel) {
                continue;
            }
            if entry.path().join("package.json").is_file() {
                if let Ok(pkg) = read_package(entry.path()) {
                    packages.push(pkg);
                }
            }
        }

        Ok(Self {
            root,
            turbo,
            sidecar,
            packages,
            pkg_manager,
        })
    }

    pub fn package(&self, name: &str) -> Option<&Package> {
        self.packages
            .iter()
            .find(|p| p.name == name || p.short == name)
    }
}

/// Either marker roots a project.
///
/// `turbo.json` was the only one procpane accepted, which made the daemon
/// unusable for the very case rung 1 of the schema exists for: a single Vite
/// app with four lines of `preceipts.toml` and no monorepo tooling at all.
pub const ROOT_MARKERS: [&str; 2] = ["preceipts.toml", "turbo.json"];

fn find_root(start: &Path) -> Result<PathBuf> {
    let start = start.canonicalize().with_context(|| "canonicalize start")?;
    let mut cur = start.as_path();
    loop {
        if ROOT_MARKERS.iter().any(|m| cur.join(m).is_file()) {
            return Ok(cur.to_path_buf());
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => {
                return Err(anyhow!(
                    "no preceipts.toml or turbo.json found from {}",
                    start.display()
                ))
            }
        }
    }
}

fn detect_package_manager(root: &Path) -> String {
    if let Ok(text) = std::fs::read_to_string(root.join("package.json")) {
        if let Ok(pj) = serde_json::from_str::<RootPackageJson>(&text) {
            if let Some(pm) = pj.package_manager.as_deref() {
                let name = pm.split_once('@').map(|(n, _)| n).unwrap_or(pm);
                return name.to_string();
            }
        }
    }
    if root.join("pnpm-lock.yaml").is_file() {
        return "pnpm".into();
    }
    if root.join("yarn.lock").is_file() {
        return "yarn".into();
    }
    if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        return "bun".into();
    }
    "npm".into()
}

fn discover_workspace_patterns(root: &Path) -> Result<Vec<String>> {
    // pnpm-workspace.yaml takes precedence if present.
    let pnpm = root.join("pnpm-workspace.yaml");
    if pnpm.is_file() {
        let text = std::fs::read_to_string(&pnpm)?;
        let ws: PnpmWorkspace =
            serde_yaml::from_str(&text).with_context(|| "parse pnpm-workspace.yaml")?;
        return Ok(ws.packages);
    }
    let pkg_path = root.join("package.json");
    if !pkg_path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&pkg_path)?;
    let root_pkg: RootPackageJson =
        serde_json::from_str(&text).with_context(|| "parse root package.json")?;
    let _ = root_pkg.name;
    let _ = root_pkg.package_manager;
    let patterns = match root_pkg.workspaces {
        Some(WorkspacesField::Plain(v)) => v,
        Some(WorkspacesField::Object { packages }) => packages,
        None => Vec::new(),
    };
    Ok(patterns)
}

fn read_package(dir: &Path) -> Result<Package> {
    let pj_path = dir.join("package.json");
    let text =
        std::fs::read_to_string(&pj_path).with_context(|| format!("read {}", pj_path.display()))?;
    let pj: PackageJson =
        serde_json::from_str(&text).with_context(|| format!("parse {}", pj_path.display()))?;
    let dir_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = pj.name.clone().unwrap_or_else(|| dir_name.clone());
    // Short alias: strip @scope/ prefix if present; else fall back to dir name.
    let short = if let Some((_, tail)) = name.split_once('/') {
        tail.to_string()
    } else if name.starts_with('@') {
        dir_name.clone()
    } else {
        name.clone()
    };
    let mut deps = Vec::new();
    for d in pj
        .dependencies
        .keys()
        .chain(pj.dev_dependencies.keys())
        .chain(pj.peer_dependencies.keys())
    {
        deps.push(d.clone());
    }
    let turbo_path = dir.join("turbo.json");
    let turbo = if turbo_path.is_file() {
        Some(TurboJson::load(&turbo_path)?)
    } else {
        None
    };
    Ok(Package {
        name,
        short,
        path: dir.to_path_buf(),
        scripts: pj.scripts,
        deps,
        turbo,
        is_root: false, // overwritten by caller for the workspace-root package
    })
}

/// A package whose "scripts" are the manifest's non-task services.
///
/// Returns `None` when the manifest declares nothing that needs one — every
/// service bound to a `task`, or no manifest at all — so a plain turborepo
/// sees no change.
///
/// A service declared with `image` gets no script: containers need a runtime
/// this project has not built yet, and inventing a shell command that pretends
/// to start one would turn a missing feature into a confusing failure. `doctor`
/// is where that is said; here it is simply absent.
fn synthetic_package(root: &Path) -> Result<Option<Package>> {
    let Some(manifest) =
        preceipts_core::manifest::load(root).map_err(|e| anyhow!("reading preceipts.toml: {e}"))?
    else {
        return Ok(None);
    };

    let scripts: BTreeMap<String, String> = manifest
        .services
        .iter()
        .filter(|service| service.task.is_none())
        .filter_map(|service| {
            service
                .run
                .as_ref()
                .map(|run| (service.name.clone(), run.clone()))
        })
        .collect();
    if scripts.is_empty() {
        return Ok(None);
    }

    // The synthetic package brings its own task definitions rather than
    // inheriting `TaskDef::default()`. A service *is* persistent — that is
    // what the word means here — and a task that defaults to non-persistent
    // is rejected by the daemon with "for non-persistent tasks use turbo run
    // directly", which is useless advice for a service turbo has never heard
    // of. Caching is off for the same reason: there is no output to cache.
    let tasks = scripts
        .keys()
        .map(|name| {
            (
                name.clone(),
                crate::config::TaskDef {
                    persistent: true,
                    cache: Some(false),
                    ..Default::default()
                },
            )
        })
        .collect();

    Ok(Some(Package {
        name: crate::bridge::SYNTHETIC_PACKAGE.to_string(),
        short: crate::bridge::SYNTHETIC_PACKAGE.to_string(),
        path: root.to_path_buf(),
        scripts,
        deps: Vec::new(),
        turbo: Some(crate::config::TurboJson {
            tasks,
            pipeline: BTreeMap::new(),
        }),
        // Not the root package: root packages are skipped by bare-name
        // expansion because their scripts are usually aggregators. These are
        // the opposite — they are the actual services.
        is_root: false,
    }))
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    /// Rung 1 of the schema: one service, no monorepo tooling at all. This is
    /// the case procpane could not serve, because it required a turbo.json to
    /// even find the root.
    #[test]
    fn a_project_can_be_rooted_by_the_manifest_alone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "preceipts.toml",
            "[services.web]\nrun = \"vite\"\nhost = \"web\"\nhealth.log = \"ready\"\n",
        );

        let project = Project::discover(root).expect("no turbo.json needed");
        assert!(project.turbo.tasks.is_empty(), "nothing came from turbo");
        assert!(
            project.sidecar.tasks.contains_key("preceipts#web"),
            "the service is there: {:?}",
            project.sidecar.tasks.keys().collect::<Vec<_>>()
        );
    }

    /// A service is persistent by definition. Inheriting turbo's default of
    /// non-persistent got it rejected with advice about `turbo run`, for a
    /// task turbo has never heard of.
    #[test]
    fn a_synthetic_service_is_persistent_and_uncached() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "preceipts.toml", "[services.web]\nrun = \"vite\"\n");

        let package = synthetic_package(root).unwrap().expect("a package");
        assert_eq!(package.scripts.get("web").map(String::as_str), Some("vite"));
        let def = package.turbo.as_ref().unwrap().task("web").unwrap();
        assert!(def.persistent, "a service does not exit");
        assert_eq!(def.cache, Some(false), "there is no output to cache");
    }

    /// A service bound to a turbo task is turbo's to start; synthesizing a
    /// second definition of it would be the drift the `task` field exists to
    /// prevent.
    #[test]
    fn a_task_backed_service_gets_no_synthetic_script() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "preceipts.toml",
            "[services.api]\ntask = \"api#dev\"\n",
        );
        assert!(synthetic_package(root).unwrap().is_none());
    }

    /// Containers have no runtime here yet. A script that pretended to start
    /// one would turn a missing feature into a confusing failure.
    #[test]
    fn a_container_service_is_absent_rather_than_faked() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "preceipts.toml",
            "[services.db]\nimage = \"postgres:17\"\n",
        );
        assert!(synthetic_package(root).unwrap().is_none());
    }

    #[test]
    fn a_plain_turborepo_gains_nothing_it_did_not_have() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "turbo.json",
            r#"{"tasks": {"dev": {"persistent": true}}}"#,
        );
        write(root, "package.json", r#"{"name": "r", "workspaces": []}"#);
        assert!(
            synthetic_package(root).unwrap().is_none(),
            "no manifest, no synthetic package"
        );
        assert!(Project::discover(root).is_ok());
    }
}

#[cfg(test)]
mod block_tests {
    use preceipts_core::ports::{Block, Reservations, BLOCK_SIZE};

    /// The property two worktrees depend on: their blocks do not overlap, so
    /// neither has to know the other exists.
    #[test]
    fn two_workspaces_of_one_project_get_disjoint_blocks() {
        let mut reservations = Reservations::default();
        let a = reservations.reserve("proj/main").unwrap();
        let b = reservations.reserve("proj/add-checkout-flow").unwrap();
        assert_ne!(a.start, b.start);
        for offset in 0..BLOCK_SIZE {
            assert!(!b.contains(a.port(offset).unwrap()));
            assert!(!a.contains(b.port(offset).unwrap()));
        }
    }

    /// And the property a bookmark depends on: the same workspace comes back
    /// to the same addresses, across restarts and across machines.
    #[test]
    fn a_workspace_returns_to_its_own_block() {
        let mut first = Reservations::default();
        let block = first.reserve("proj/main").unwrap();
        assert_eq!(first.reserve("proj/main"), Some(block), "within one run");

        let mut second = Reservations::default();
        assert_eq!(
            second.reserve("proj/main"),
            Some(block),
            "and on a machine that has never seen this workspace"
        );
    }

    /// Offset 0 is the workspace's TLS proxy; services start at 1. A service
    /// landing on the proxy's port would be a listener collision inside one
    /// workspace, which is the bug the blocks exist to prevent between them.
    #[test]
    fn the_proxy_port_is_not_handed_to_a_service() {
        let block = Block { start: 21000 };
        assert_eq!(block.port(0), Some(21000));
        assert_eq!(block.port(1), Some(21001));
        assert_eq!(block.port(BLOCK_SIZE), None, "the block has an end");
    }
}

#[cfg(test)]
mod offset_tests {
    use std::collections::BTreeMap;

    /// The promise: a service keeps its port as long as the manifest does.
    ///
    /// Offsets are assigned by iterating hostnames in sorted order rather
    /// than in graph order, because graph order follows a work stack seeded
    /// by however `up` was invoked. This pins the rule that makes the promise
    /// true — with one service it is trivially satisfied and proves nothing.
    #[test]
    fn service_offsets_do_not_depend_on_the_order_tasks_were_requested() {
        let assign = |ids: &[&str]| -> Vec<(String, u16)> {
            // The daemon keys hostnames in a BTreeMap; that is the ordering
            // under test, so the map is what the test builds.
            let hostnames: BTreeMap<String, String> = ids
                .iter()
                .map(|id| (id.to_string(), format!("{id}.localhost")))
                .collect();
            hostnames
                .keys()
                .enumerate()
                .map(|(offset, id)| (id.clone(), 21000 + offset as u16 + 1))
                .collect()
        };

        let one = assign(&["preceipts#api", "preceipts#db", "preceipts#web"]);
        let another = assign(&["preceipts#web", "preceipts#api", "preceipts#db"]);
        assert_eq!(one, another, "the request order must not move a port");
        assert_eq!(one[0], ("preceipts#api".to_string(), 21001));
        assert_eq!(one[2], ("preceipts#web".to_string(), 21003));
    }
}
