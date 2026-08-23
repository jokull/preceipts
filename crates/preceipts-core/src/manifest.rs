//! `preceipts.toml` — how a project describes its sandbox.
//!
//! Designed in docs/direction-2026-08.md against trip's `packages/sandbox`,
//! which is the honest upper bound: a Postgres, three app servers, provider
//! mocks, seeded actors, an email outbox, an analytics ledger, and a local
//! error sink. The rule that keeps that from turning into a program:
//!
//! > **If the app or a receipt must understand it, it is schema. If only the
//! > project understands it, it is an action.**
//!
//! So services, env policy, readiness, mocks, drains, fidelity, and seeds are
//! modelled here — and everything domain-specific (`purchase-smoke`,
//! `provider-contracts`, `auth <actor>`) is a declared verb that becomes a CLI
//! command, an MCP tool, and a panel button without us knowing what it does.
//!
//! Progressive disclosure is the other rule: every rung is optional. A repo
//! with a `dev` script needs no file at all.

use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::path::Path;

pub const MANIFEST_FILE: &str = "preceipts.toml";
pub const LOCAL_MANIFEST_FILE: &str = "preceipts.local.toml";

/// How real a service or provider is. Adopted verbatim from trip's
/// `SandboxBindingFidelity`, because the vocabulary is already right.
///
/// This exists so an agent cannot over-claim: a browse against a shimmed
/// payment provider is route proof, not payment proof. It is also what a
/// receipt records about the environment it was minted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fidelity {
    /// A real implementation running locally — a real Postgres.
    #[default]
    LocalReal,
    /// Local, but standing in for the real thing — a shim.
    LocalSimulated,
    /// Served from fixtures through our proxy.
    Mocked,
    /// Switched off entirely.
    Disabled,
    /// Needs a remote we deliberately do not call.
    RemoteRequired,
}

impl Fidelity {
    pub fn as_str(self) -> &'static str {
        match self {
            Fidelity::LocalReal => "local-real",
            Fidelity::LocalSimulated => "local-simulated",
            Fidelity::Mocked => "mocked",
            Fidelity::Disabled => "disabled",
            Fidelity::RemoteRequired => "remote-required",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "local-real" => Fidelity::LocalReal,
            "local-simulated" => Fidelity::LocalSimulated,
            "mocked" => Fidelity::Mocked,
            "disabled" => Fidelity::Disabled,
            "remote-required" => Fidelity::RemoteRequired,
            _ => return None,
        })
    }
}

/// Where a service runs. Per service, not per project — the practical answer
/// for a repo like trip is to containerise the stateful things and run the JS
/// natively, where iteration speed lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Runtime {
    /// A process on the host. No VM, no container.
    #[default]
    Native,
    /// Host process, confined by a Seatbelt profile and an egress allowlist.
    Confined,
    /// Delegated to a container runtime.
    Container,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Native => "native",
            Runtime::Confined => "confined",
            Runtime::Container => "container",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "native" => Runtime::Native,
            "confined" => Runtime::Confined,
            "container" => Runtime::Container,
            _ => return None,
        })
    }
}

/// When a service counts as ready. "The process exists" is a lie that costs
/// downstream services a race, which is why there is no such variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// A port is accepting connections.
    Tcp(u16),
    /// An HTTP path answers 2xx.
    Http(String),
    /// A line matching this regex appears in the service's output. Dev-native:
    /// the thing Vite and Next actually tell you.
    Log(String),
    /// Nothing declared — the service is ready when it has started, which is
    /// only honest for one-shot commands.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// Command to run. Mutually exclusive with `image`.
    pub run: Option<String>,
    /// Container image. Implies `runtime = "container"` unless overridden.
    pub image: Option<String>,
    pub runtime: Runtime,
    pub health: Health,
    /// Services that must be healthy first.
    pub needs: Vec<String>,
    /// Hostname *label*, not a fully-qualified name: the fabric composes
    /// `<host>.<workspace>.<project>.test`. Authors never spell a domain.
    pub host: Option<String>,
    /// Env keys and `@group` references this service may see. Per-service by
    /// design, so a stray `postinstall` cannot read your Stripe key.
    pub env: Vec<String>,
    /// Values captured from this service's output and exported to dependents.
    /// `stripe listen` printing a `whsec_…` is the motivating case.
    pub capture: BTreeMap<String, String>,
    pub fidelity: Fidelity,
}

/// A named set of env keys, so twenty repeated names become one `@shared`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvGroup {
    pub name: String,
    /// Key names, `*` wildcards allowed (`SENTRY_*`).
    pub keys: Vec<String>,
}

/// A constraint on an env value. Not an allowlist — a policy with assertions,
/// because trip learned that "only copy these keys" is not enough when one of
/// them might be a live Stripe key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvRule {
    pub key: String,
    /// The value must start with this, or the workspace refuses to boot.
    pub require_prefix: Option<String>,
}

/// A side-effect sink. What turns "the screenshot looks right" into evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drain {
    pub name: String,
    /// `outbox`, `jsonl`, `sentry`, … — free-form, since the project owns the
    /// meaning and we own only the sweep.
    pub kind: String,
    pub from: Option<String>,
}

/// An upstream served from fixtures instead of called for real.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mock {
    pub name: String,
    /// URL pattern this intercepts.
    pub match_pattern: String,
    pub fixtures: Option<String>,
    /// Record intercepted requests into the workspace's HTTP transcript.
    pub record: bool,
}

/// A project-declared verb. Becomes a CLI command, an MCP tool, and a button —
/// which is how the kitchen sink stays authorable without us modelling it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub name: String,
    pub run: String,
    pub about: Option<String>,
    /// Positional argument names, substituted as `{{name}}`.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub name: String,
    pub run: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub schema: u32,
    pub services: Vec<Service>,
    pub env_groups: Vec<EnvGroup>,
    pub env_rules: Vec<EnvRule>,
    /// Commands run once when a workspace is created.
    pub setup: Vec<String>,
    pub bundles: Vec<Bundle>,
    /// Named shortcuts over bundle sets; `@name` references another radius.
    pub radii: BTreeMap<String, Vec<String>>,
    pub drains: Vec<Drain>,
    pub mocks: Vec<Mock>,
    pub actions: Vec<Action>,
    /// Services that must be healthy before anything is browsed. A composite
    /// gate above per-service health: "every process is up" and "you may take
    /// a screenshot" are different claims.
    pub ready: Vec<String>,
}

fn err(message: impl AsRef<str>) -> Error {
    Error::Git(git2::Error::from_str(message.as_ref()))
}

impl Manifest {
    pub fn service(&self, name: &str) -> Option<&Service> {
        self.services.iter().find(|s| s.name == name)
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    /// Resolve a radius to its bundle names, following `@radius` references.
    pub fn resolve_radius(&self, name: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut seen = Vec::new();
        self.resolve_radius_into(name, &mut out, &mut seen)?;
        Ok(out)
    }

    fn resolve_radius_into(
        &self,
        name: &str,
        out: &mut Vec<String>,
        seen: &mut Vec<String>,
    ) -> Result<()> {
        if seen.iter().any(|s| s == name) {
            return Err(err(format!("radius \"{name}\" refers to itself")));
        }
        seen.push(name.to_string());
        let entries = self
            .radii
            .get(name)
            .ok_or_else(|| err(format!("no radius \"{name}\"")))?;
        for entry in entries {
            match entry.strip_prefix('@') {
                Some(other) => self.resolve_radius_into(other, out, seen)?,
                None => {
                    if !out.contains(entry) {
                        out.push(entry.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// Services in dependency order. Errors on a cycle rather than hanging or
    /// silently dropping one.
    pub fn boot_order(&self) -> Result<Vec<&Service>> {
        let mut ordered: Vec<&Service> = Vec::new();
        let mut state: BTreeMap<&str, u8> = BTreeMap::new(); // 1 = visiting, 2 = done

        fn visit<'a>(
            manifest: &'a Manifest,
            service: &'a Service,
            state: &mut BTreeMap<&'a str, u8>,
            ordered: &mut Vec<&'a Service>,
        ) -> Result<()> {
            match state.get(service.name.as_str()) {
                Some(2) => return Ok(()),
                Some(1) => {
                    return Err(err(format!(
                        "services form a dependency cycle through \"{}\"",
                        service.name
                    )))
                }
                _ => {}
            }
            state.insert(&service.name, 1);
            for need in &service.needs {
                let dependency = manifest.service(need).ok_or_else(|| {
                    err(format!(
                        "service \"{}\" needs \"{need}\", which is not defined",
                        service.name
                    ))
                })?;
                visit(manifest, dependency, state, ordered)?;
            }
            state.insert(&service.name, 2);
            ordered.push(service);
            Ok(())
        }

        for service in &self.services {
            visit(self, service, &mut state, &mut ordered)?;
        }
        Ok(ordered)
    }

    /// Problems worth telling someone about before they hit them at runtime.
    /// This is what `preceipts doctor` reports.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if let Err(error) = self.boot_order() {
            problems.push(error.to_string());
        }

        for service in &self.services {
            if service.run.is_none() && service.image.is_none() {
                problems.push(format!(
                    "service \"{}\" has neither run nor image — nothing to start",
                    service.name
                ));
            }
            if service.run.is_some() && service.image.is_some() {
                problems.push(format!(
                    "service \"{}\" has both run and image — pick one",
                    service.name
                ));
            }
            if service.host.is_some() && service.health == Health::None {
                problems.push(format!(
                    "service \"{}\" publishes a URL but declares no health check, \
                     so nothing can wait for it",
                    service.name
                ));
            }
            for key in &service.env {
                if let Some(group) = key.strip_prefix('@') {
                    if !self.env_groups.iter().any(|g| g.name == group) {
                        problems.push(format!(
                            "service \"{}\" references env group \"@{group}\", which is not defined",
                            service.name
                        ));
                    }
                }
            }
        }

        for name in &self.ready {
            if self.service(name).is_none() {
                problems.push(format!("[ready] names \"{name}\", which is not a service"));
            }
        }

        for (name, _) in self.radii.iter() {
            if let Err(error) = self.resolve_radius(name) {
                problems.push(error.to_string());
            }
        }

        problems
    }
}

/// Load `preceipts.toml`, with `preceipts.local.toml` layered on top when it
/// exists. The local file is personal and gitignored — it is how you point one
/// service at something else without editing the shared manifest.
pub fn load(root: &Path) -> Result<Option<Manifest>> {
    let path = root.join(MANIFEST_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let mut manifest = parse(&text)?;

    if let Ok(local) = std::fs::read_to_string(root.join(LOCAL_MANIFEST_FILE)) {
        let overlay = parse(&local)?;
        merge(&mut manifest, overlay);
    }
    Ok(Some(manifest))
}

/// Later definitions win, per name; everything else is appended.
fn merge(base: &mut Manifest, overlay: Manifest) {
    for service in overlay.services {
        match base.services.iter_mut().find(|s| s.name == service.name) {
            Some(existing) => *existing = service,
            None => base.services.push(service),
        }
    }
    for action in overlay.actions {
        match base.actions.iter_mut().find(|a| a.name == action.name) {
            Some(existing) => *existing = action,
            None => base.actions.push(action),
        }
    }
    for group in overlay.env_groups {
        match base.env_groups.iter_mut().find(|g| g.name == group.name) {
            Some(existing) => *existing = group,
            None => base.env_groups.push(group),
        }
    }
    if !overlay.ready.is_empty() {
        base.ready = overlay.ready;
    }
    if !overlay.setup.is_empty() {
        base.setup = overlay.setup;
    }
}

pub fn parse(text: &str) -> Result<Manifest> {
    let doc: toml::Table =
        toml::from_str(text).map_err(|e| err(format!("cannot parse {MANIFEST_FILE}: {e}")))?;

    let schema = doc.get("schema").and_then(|v| v.as_integer()).unwrap_or(1) as u32;
    if schema != 1 {
        return Err(err(format!(
            "{MANIFEST_FILE} declares schema = {schema}, but this build understands 1"
        )));
    }

    let mut manifest = Manifest {
        schema,
        ..Default::default()
    };

    if let Some(table) = doc.get("services").and_then(|v| v.as_table()) {
        for (name, value) in table {
            manifest.services.push(parse_service(name, value)?);
        }
    }

    if let Some(table) = doc.get("env").and_then(|v| v.as_table()) {
        for (name, value) in table {
            let entry = value
                .as_table()
                .ok_or_else(|| err(format!("[env.{name}] must be a table")))?;
            if let Some(keys) = entry.get("keys").and_then(|v| v.as_array()) {
                manifest.env_groups.push(EnvGroup {
                    name: name.clone(),
                    keys: keys
                        .iter()
                        .filter_map(|k| k.as_str().map(str::to_string))
                        .collect(),
                });
            } else {
                manifest.env_rules.push(EnvRule {
                    key: name.clone(),
                    require_prefix: entry
                        .get("require_prefix")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });
            }
        }
    }

    if let Some(setup) = doc.get("setup").and_then(|v| v.get("run")) {
        manifest.setup = string_list(setup, "setup.run")?;
    }

    if let Some(table) = doc.get("bundles").and_then(|v| v.as_table()) {
        for (name, value) in table {
            let run = value
                .get("run")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(format!("[bundles.{name}] needs a run string")))?;
            manifest.bundles.push(Bundle {
                name: name.clone(),
                run: run.to_string(),
            });
        }
    }

    if let Some(table) = doc.get("radii").and_then(|v| v.as_table()) {
        for (name, value) in table {
            manifest
                .radii
                .insert(name.clone(), string_list(value, &format!("radii.{name}"))?);
        }
    }

    if let Some(table) = doc.get("drains").and_then(|v| v.as_table()) {
        for (name, value) in table {
            let kind = value
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(format!("[drains.{name}] needs a kind")))?;
            manifest.drains.push(Drain {
                name: name.clone(),
                kind: kind.to_string(),
                from: value
                    .get("from")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
    }

    if let Some(table) = doc.get("mocks").and_then(|v| v.as_table()) {
        for (name, value) in table {
            let pattern = value
                .get("match")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(format!("[mocks.{name}] needs a match pattern")))?;
            manifest.mocks.push(Mock {
                name: name.clone(),
                match_pattern: pattern.to_string(),
                fixtures: value
                    .get("fixtures")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                record: value
                    .get("record")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            });
        }
    }

    if let Some(table) = doc.get("actions").and_then(|v| v.as_table()) {
        for (name, value) in table {
            let run = value
                .get("run")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err(format!("[actions.{name}] needs a run string")))?;
            manifest.actions.push(Action {
                name: name.clone(),
                run: run.to_string(),
                about: value
                    .get("about")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                args: value
                    .get("args")
                    .map(|v| string_list(v, &format!("actions.{name}.args")))
                    .transpose()?
                    .unwrap_or_default(),
            });
        }
    }

    if let Some(requires) = doc.get("ready").and_then(|v| v.get("requires")) {
        manifest.ready = string_list(requires, "ready.requires")?;
    }

    manifest.services.sort_by(|a, b| a.name.cmp(&b.name));
    manifest.actions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(manifest)
}

fn parse_service(name: &str, value: &toml::Value) -> Result<Service> {
    let table = value
        .as_table()
        .ok_or_else(|| err(format!("[services.{name}] must be a table")))?;

    let image = table
        .get("image")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let runtime = match table.get("runtime").and_then(|v| v.as_str()) {
        Some(text) => Runtime::parse(text).ok_or_else(|| {
            err(format!(
                "service \"{name}\" has runtime = \"{text}\"; expected native, confined, or container"
            ))
        })?,
        // An image without an explicit runtime means a container — saying both
        // would be noise.
        None if image.is_some() => Runtime::Container,
        None => Runtime::Native,
    };

    let health = match table.get("health") {
        None => Health::None,
        Some(health) => {
            let health = health
                .as_table()
                .ok_or_else(|| err(format!("service \"{name}\": health must be a table")))?;
            if let Some(port) = health.get("tcp").and_then(|v| v.as_integer()) {
                Health::Tcp(port as u16)
            } else if let Some(path) = health.get("http").and_then(|v| v.as_str()) {
                Health::Http(path.to_string())
            } else if let Some(pattern) = health.get("log").and_then(|v| v.as_str()) {
                Health::Log(pattern.to_string())
            } else {
                return Err(err(format!(
                    "service \"{name}\": health needs one of tcp, http, or log"
                )));
            }
        }
    };

    let fidelity = match table.get("fidelity").and_then(|v| v.as_str()) {
        Some(text) => Fidelity::parse(text).ok_or_else(|| {
            err(format!(
                "service \"{name}\" has an unknown fidelity \"{text}\""
            ))
        })?,
        None => Fidelity::default(),
    };

    let capture = table
        .get("capture")
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    Ok(Service {
        name: name.to_string(),
        run: table
            .get("run")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        image,
        runtime,
        health,
        needs: table
            .get("needs")
            .map(|v| string_list(v, &format!("services.{name}.needs")))
            .transpose()?
            .unwrap_or_default(),
        host: table
            .get("host")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        env: table
            .get("env")
            .map(|v| string_list(v, &format!("services.{name}.env")))
            .transpose()?
            .unwrap_or_default(),
        capture,
        fidelity,
    })
}

fn string_list(value: &toml::Value, what: &str) -> Result<Vec<String>> {
    let array = value
        .as_array()
        .ok_or_else(|| err(format!("{what} must be an array of strings")))?;
    array
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| err(format!("{what} must contain only strings")))
        })
        .collect()
}

/// Rung 0: guess a single service from what the repository obviously is.
///
/// Detection exists so `preceipts new` works in a repo that has never heard of
/// us. The file appears when you outgrow the guess, not before.
pub fn detect(root: &Path) -> Option<Manifest> {
    let (run, health) = if root.join("package.json").is_file() {
        let text = std::fs::read_to_string(root.join("package.json")).ok()?;
        let json: serde_json::Value = serde_json::from_str(&text).ok()?;
        let scripts = json.get("scripts")?;
        let script = ["dev", "start", "serve"]
            .into_iter()
            .find(|name| scripts.get(name).is_some())?;
        let manager = if root.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if root.join("bun.lockb").is_file() {
            "bun"
        } else if root.join("yarn.lock").is_file() {
            "yarn"
        } else {
            "npm run"
        };
        (
            format!("{manager} {script}"),
            // Vite, Next, and most dev servers announce themselves; matching
            // the announcement beats guessing a port.
            Health::Log("ready|Ready|listening|Local:".to_string()),
        )
    } else if root.join("Cargo.toml").is_file() {
        ("cargo run".to_string(), Health::None)
    } else {
        return None;
    };

    Some(Manifest {
        schema: 1,
        services: vec![Service {
            name: "app".to_string(),
            run: Some(run),
            image: None,
            runtime: Runtime::Native,
            health,
            needs: Vec::new(),
            host: Some("app".to_string()),
            env: Vec::new(),
            capture: BTreeMap::new(),
            fidelity: Fidelity::LocalReal,
        }],
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_four_line_manifest_is_enough() {
        let manifest = parse(
            r#"
[services.web]
run = "pnpm dev"
health.log = "ready in"
"#,
        )
        .unwrap();
        let web = manifest.service("web").unwrap();
        assert_eq!(web.run.as_deref(), Some("pnpm dev"));
        assert_eq!(web.health, Health::Log("ready in".into()));
        assert_eq!(web.runtime, Runtime::Native);
        assert!(manifest.problems().is_empty());
    }

    #[test]
    fn an_image_implies_a_container_without_saying_so_twice() {
        let manifest = parse(
            r#"
[services.db]
image = "postgres:17"
health.tcp = 5432
"#,
        )
        .unwrap();
        let db = manifest.service("db").unwrap();
        assert_eq!(db.runtime, Runtime::Container);
        assert_eq!(db.health, Health::Tcp(5432));
    }

    #[test]
    fn runtime_is_per_service_so_a_stack_can_be_mixed() {
        let manifest = parse(
            r#"
[services.db]
image = "postgres:17"
health.tcp = 5432

[services.api]
run = "pnpm --filter api dev"
runtime = "native"
health.http = "/health"
needs = ["db"]
"#,
        )
        .unwrap();
        assert_eq!(manifest.service("db").unwrap().runtime, Runtime::Container);
        assert_eq!(manifest.service("api").unwrap().runtime, Runtime::Native);
    }

    #[test]
    fn services_boot_in_dependency_order() {
        let manifest = parse(
            r#"
[services.next]
run = "x"
needs = ["api"]
[services.api]
run = "x"
needs = ["db"]
[services.db]
run = "x"
"#,
        )
        .unwrap();
        let order: Vec<&str> = manifest
            .boot_order()
            .unwrap()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(order, ["db", "api", "next"]);
    }

    #[test]
    fn a_dependency_cycle_is_reported_not_hung_on() {
        let manifest = parse(
            r#"
[services.a]
run = "x"
needs = ["b"]
[services.b]
run = "x"
needs = ["a"]
"#,
        )
        .unwrap();
        assert!(manifest.boot_order().is_err());
        assert!(manifest.problems().iter().any(|p| p.contains("cycle")));
    }

    #[test]
    fn a_missing_dependency_names_itself() {
        let manifest = parse("[services.api]\nrun = \"x\"\nneeds = [\"ghost\"]\n").unwrap();
        let problems = manifest.problems();
        assert!(problems.iter().any(|p| p.contains("ghost")), "{problems:?}");
    }

    /// The authoring win over trip's hundred lines of repeated `env_from`.
    #[test]
    fn env_groups_collapse_repetition_and_rules_assert() {
        let manifest = parse(
            r#"
[env.shared]
keys = ["ENV", "SECRET", "SENTRY_*"]

[env.STRIPE_SECRET_KEY]
require_prefix = "sk_test_"

[services.api]
run = "x"
env = ["@shared", "STRIPE_SECRET_KEY"]
"#,
        )
        .unwrap();
        assert_eq!(manifest.env_groups[0].keys.len(), 3);
        assert_eq!(
            manifest.env_rules[0].require_prefix.as_deref(),
            Some("sk_test_")
        );
        assert!(manifest.problems().is_empty());
    }

    #[test]
    fn an_undefined_env_group_is_caught_before_boot() {
        let manifest = parse("[services.api]\nrun = \"x\"\nenv = [\"@nope\"]\n").unwrap();
        assert!(manifest.problems().iter().any(|p| p.contains("@nope")));
    }

    /// The stripe-webhook case: a service mints a value its dependents need.
    #[test]
    fn a_service_can_capture_env_for_its_dependents() {
        let manifest = parse(
            r#"
[services.stripe-webhook]
run = "stripe listen"
capture.STRIPE_WEBHOOK_SECRET = "whsec_\\w+"

[services.api]
run = "x"
needs = ["stripe-webhook"]
"#,
        )
        .unwrap();
        let hook = manifest.service("stripe-webhook").unwrap();
        assert_eq!(
            hook.capture
                .get("STRIPE_WEBHOOK_SECRET")
                .map(String::as_str),
            Some("whsec_\\w+")
        );
    }

    #[test]
    fn radii_are_shortcuts_over_bundles_and_compose() {
        let manifest = parse(
            r#"
[radii]
s = ["admin-user", "customer-user"]
m = ["@s", "trip"]
l = ["@m", "checkout"]
"#,
        )
        .unwrap();
        assert_eq!(
            manifest.resolve_radius("l").unwrap(),
            ["admin-user", "customer-user", "trip", "checkout"]
        );
    }

    #[test]
    fn a_self_referential_radius_is_refused() {
        let manifest = parse("[radii]\na = [\"@b\"]\nb = [\"@a\"]\n").unwrap();
        assert!(manifest.resolve_radius("a").is_err());
        assert!(!manifest.problems().is_empty());
    }

    #[test]
    fn drains_mocks_and_actions_are_carried_whole() {
        let manifest = parse(
            r#"
[drains.email]
kind = "outbox"
from = "/artifacts/emails"

[mocks.bokun]
match = "https://api.bokun.io/**"
fixtures = "sandbox/mocks/bokun"
record = true

[actions.auth]
run = "pnpm seed:auth {{actor}}"
args = ["actor"]
about = "Switch the browser to a seeded actor"
"#,
        )
        .unwrap();
        assert_eq!(manifest.drains[0].kind, "outbox");
        assert!(manifest.mocks[0].record);
        let auth = manifest.action("auth").unwrap();
        assert_eq!(auth.args, ["actor"]);
        assert_eq!(
            auth.about.as_deref(),
            Some("Switch the browser to a seeded actor")
        );
    }

    #[test]
    fn fidelity_uses_trips_vocabulary() {
        let manifest = parse(
            r#"
[services.stripe]
run = "x"
fidelity = "local-simulated"
"#,
        )
        .unwrap();
        assert_eq!(
            manifest.service("stripe").unwrap().fidelity,
            Fidelity::LocalSimulated
        );
        assert_eq!(Fidelity::LocalSimulated.as_str(), "local-simulated");
    }

    #[test]
    fn an_unknown_fidelity_is_refused_rather_than_defaulted() {
        let error = parse("[services.x]\nrun = \"x\"\nfidelity = \"pretty-real\"\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("pretty-real"), "{error}");
    }

    /// Readiness is a claim above health, and naming a non-service is the
    /// mistake most likely to make a gate silently useless.
    #[test]
    fn the_ready_gate_must_name_real_services() {
        let manifest =
            parse("[services.api]\nrun = \"x\"\n\n[ready]\nrequires = [\"api\", \"ghost\"]\n")
                .unwrap();
        assert_eq!(manifest.ready, ["api", "ghost"]);
        assert!(manifest.problems().iter().any(|p| p.contains("ghost")));
    }

    #[test]
    fn a_url_without_a_health_check_is_flagged() {
        let manifest = parse("[services.web]\nrun = \"x\"\nhost = \"web\"\n").unwrap();
        assert!(manifest
            .problems()
            .iter()
            .any(|p| p.contains("no health check")));
    }

    #[test]
    fn a_service_with_nothing_to_start_is_flagged() {
        let manifest = parse("[services.ghost]\nhost = \"g\"\n").unwrap();
        assert!(manifest
            .problems()
            .iter()
            .any(|p| p.contains("neither run nor image")));
    }

    #[test]
    fn a_future_schema_is_refused_rather_than_guessed_at() {
        let error = parse("schema = 2\n").unwrap_err().to_string();
        assert!(error.contains("schema = 2"), "{error}");
    }

    #[test]
    fn the_local_overlay_wins_per_name() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(MANIFEST_FILE),
            "[services.api]\nrun = \"real\"\n\n[services.db]\nrun = \"db\"\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join(LOCAL_MANIFEST_FILE),
            "[services.api]\nrun = \"mine\"\n",
        )
        .unwrap();

        let manifest = load(temp.path()).unwrap().unwrap();
        assert_eq!(
            manifest.service("api").unwrap().run.as_deref(),
            Some("mine")
        );
        assert_eq!(
            manifest.service("db").unwrap().run.as_deref(),
            Some("db"),
            "untouched services survive the overlay"
        );
    }

    #[test]
    fn a_repo_without_a_manifest_is_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load(temp.path()).unwrap().is_none());
    }

    #[test]
    fn detection_covers_the_no_file_rung() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        std::fs::write(temp.path().join("pnpm-lock.yaml"), "").unwrap();

        let manifest = detect(temp.path()).unwrap();
        let app = manifest.service("app").unwrap();
        assert_eq!(app.run.as_deref(), Some("pnpm dev"));
        assert!(matches!(app.health, Health::Log(_)));
        assert!(manifest.problems().is_empty());
    }

    #[test]
    fn detection_declines_when_it_cannot_tell() {
        let temp = tempfile::tempdir().unwrap();
        assert!(detect(temp.path()).is_none());
        std::fs::write(temp.path().join("package.json"), r#"{"name":"x"}"#).unwrap();
        assert!(detect(temp.path()).is_none(), "no dev script, no guess");
    }
}
