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
    /// Command to run. Mutually exclusive with `image` and `task`.
    pub run: Option<String>,
    /// A task in the repository's own runner — `api#dev` for turbo — run
    /// through it rather than spawned directly.
    ///
    /// This is what lets a monorepo keep one definition of how a package
    /// starts. Writing `run = "pnpm --filter api dev"` next to a turbo.json
    /// that already says so is a second copy waiting to drift.
    pub task: Option<String>,
    /// Container image. Implies `runtime = "container"` unless overridden.
    pub image: Option<String>,
    pub runtime: Runtime,
    pub health: Health,
    /// Services that must be healthy first.
    pub needs: Vec<String>,
    /// Hostname *label*, not a fully-qualified name: the fabric composes
    /// `<host>.<workspace>.<project>.localhost`. Authors never spell a domain.
    pub host: Option<String>,
    /// Env keys and `@group` references this service may see. Per-service by
    /// design, so a stray `postinstall` cannot read your Stripe key.
    pub env: Vec<String>,
    /// Values captured from this service's output and exported to dependents.
    /// `stripe listen` printing a `whsec_…` is the motivating case.
    pub capture: BTreeMap<String, String>,
    pub fidelity: Fidelity,
    /// How long to wait before the first health probe.
    ///
    /// Not a nicety: a service that logs its readiness line during startup
    /// can be probed before it means it, and a TCP check can catch a port
    /// that is bound but not yet serving. trip sets 5s on three services for
    /// exactly this reason, and a schema that could not express it would lose
    /// that on conversion.
    pub health_start_period: Option<std::time::Duration>,
    /// Seconds between probes.
    pub health_interval: Option<std::time::Duration>,
    /// How long one probe may take.
    pub health_timeout: Option<std::time::Duration>,
    /// Signal sent to stop this service. `SIGINT` by default, because dev
    /// servers overwhelmingly treat it as "shut down cleanly".
    pub stop_signal: Option<String>,
    /// How long the service gets to stop before it is killed.
    pub stop_grace: Option<std::time::Duration>,
}

impl Service {
    /// What actually starts this service, whichever way it was declared.
    pub fn command(&self) -> Option<&str> {
        self.run.as_deref().or(self.task.as_deref())
    }
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
/// Where a value comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EnvSource {
    /// The macOS Keychain, by reference. The default, because a value worth
    /// declaring is usually a value worth not writing down.
    #[default]
    Keychain,
    /// A dotenv file in the worktree.
    Dotenv,
    /// Minted at boot by a service that exports it (Rung 3b).
    Captured,
    /// Written in the manifest itself. Fine for a URL, never for a secret.
    Literal,
}

impl EnvSource {
    pub fn as_str(self) -> &'static str {
        match self {
            EnvSource::Keychain => "keychain",
            EnvSource::Dotenv => "dotenv",
            EnvSource::Captured => "captured",
            EnvSource::Literal => "literal",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "keychain" => EnvSource::Keychain,
            "dotenv" => EnvSource::Dotenv,
            "captured" => EnvSource::Captured,
            "literal" => EnvSource::Literal,
            _ => return None,
        })
    }

    /// True when the value is a secret by construction.
    ///
    /// A secret in a build-cache key is two problems at once: every machine
    /// misses cache forever because every machine's value differs, and the
    /// value itself becomes part of a key that travels to a shared remote.
    pub fn is_secret(self) -> bool {
        matches!(self, EnvSource::Keychain)
    }
}

/// A constraint on an env value, plus the two facts about it that matter
/// outside the boot: where it comes from, and whether it belongs in a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvRule {
    pub key: String,
    /// The value must start with this, or the workspace refuses to boot.
    pub require_prefix: Option<String>,
    /// The value itself, for `from = "literal"` only.
    ///
    /// A dev container usually needs one or two settings that are not secret
    /// and not worth a Keychain round trip — `POSTGRES_PASSWORD` for a
    /// throwaway database is the canonical case. Refusing to hold those would
    /// mean either a Keychain entry per developer for a value everyone knows,
    /// or a `.env` file nobody declared.
    ///
    /// Only ever read for literals. A value written next to `from =
    /// "keychain"` is a secret in a tracked file, and the manifest refuses it
    /// rather than quietly using it.
    pub value: Option<String>,
    pub source: EnvSource,
    /// Does changing this value change the answer?
    ///
    /// `Some(true)` means it belongs in turbo's `env`/`globalEnv`, where a
    /// change busts the cache. `Some(false)` means `passThroughEnv` — reaches
    /// the process, never touches the hash. `None` means the author has not
    /// said, and a secret's default is `false` because the alternative is
    /// actively harmful.
    pub hash: Option<bool>,
}

impl EnvRule {
    /// Whether this value should contribute to a build-cache key, falling back
    /// to what its source implies when the author has not said.
    pub fn hashed(&self) -> bool {
        self.hash.unwrap_or(!self.source.is_secret())
    }
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
            let declared = [
                service.run.is_some(),
                service.task.is_some(),
                service.image.is_some(),
            ]
            .iter()
            .filter(|d| **d)
            .count();
            if declared == 0 {
                problems.push(format!(
                    "service \"{}\" has none of run, task, or image — nothing to start",
                    service.name
                ));
            }
            if declared > 1 {
                problems.push(format!(
                    "service \"{}\" declares more than one of run, task, and image — pick one",
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
                let source = match entry.get("from").and_then(|v| v.as_str()) {
                    None => EnvSource::default(),
                    Some(text) => EnvSource::parse(text).ok_or_else(|| {
                        err(format!(
                            "[env.{name}].from is \"{text}\" — use keychain, dotenv, \
                             captured, or literal"
                        ))
                    })?,
                };
                let hash =
                    match entry.get("hash") {
                        None => None,
                        Some(value) => Some(value.as_bool().ok_or_else(|| {
                            err(format!("[env.{name}].hash must be true or false"))
                        })?),
                    };
                let value = entry
                    .get("value")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if value.is_some() && source != EnvSource::Literal {
                    return Err(err(format!(
                        "[env.{name}] has a value but comes from {}; a value written in \
                         the manifest is only meaningful for from = \"literal\", and \
                         next to a secret it is a secret in a tracked file",
                        source.as_str()
                    )));
                }
                manifest.env_rules.push(EnvRule {
                    key: name.clone(),
                    value,
                    require_prefix: entry
                        .get("require_prefix")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    source,
                    hash,
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
                // Timing keys alone are not a check. Saying so explicitly
                // beats a service that declares `health.start_period` and
                // silently gets no health check at all.
                return Err(err(format!(
                    "service \"{name}\": health needs one of tcp, http, or log"
                )));
            }
        }
    };

    let duration_field = |key: &str| -> Result<Option<std::time::Duration>> {
        let Some(value) = table.get("health").and_then(|h| h.get(key)) else {
            return Ok(None);
        };
        let text = value.as_str().ok_or_else(|| {
            err(format!(
                "service \"{name}\": health.{key} must be a string like \"5s\""
            ))
        })?;
        crate::checks::parse_duration(text)
            .map(Some)
            .map_err(|e| err(format!("service \"{name}\": health.{key} {e}")))
    };
    let health_start_period = duration_field("start_period")?;
    let health_interval = duration_field("interval")?;
    let health_timeout = duration_field("timeout")?;

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

    let stop_grace = match table.get("stop_grace") {
        None => None,
        Some(value) => {
            let text = value.as_str().ok_or_else(|| {
                err(format!(
                    "service \"{name}\": stop_grace must be a string like \"30s\""
                ))
            })?;
            Some(
                crate::checks::parse_duration(text)
                    .map_err(|e| err(format!("service \"{name}\": stop_grace {e}")))?,
            )
        }
    };
    if let Some(signal) = table.get("stop_signal").and_then(|v| v.as_str()) {
        if !matches!(signal, "SIGINT" | "SIGTERM" | "SIGHUP" | "SIGQUIT") {
            return Err(err(format!(
                "service \"{name}\": stop_signal \"{signal}\" is not one of SIGINT, \
                 SIGTERM, SIGHUP, SIGQUIT"
            )));
        }
    }

    Ok(Service {
        name: name.to_string(),
        run: table
            .get("run")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        task: table
            .get("task")
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
        health_start_period,
        health_interval,
        health_timeout,
        stop_signal: table
            .get("stop_signal")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        stop_grace,
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
            task: None,
            image: None,
            runtime: Runtime::Native,
            health,
            needs: Vec::new(),
            host: Some("app".to_string()),
            env: Vec::new(),
            capture: BTreeMap::new(),
            fidelity: Fidelity::LocalReal,
            health_start_period: None,
            health_interval: None,
            health_timeout: None,
            stop_signal: None,
            stop_grace: None,
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
            .any(|p| p.contains("none of run, task, or image")));
    }

    /// A monorepo already says how a package starts; repeating it in the
    /// manifest is a second copy waiting to drift.
    #[test]
    fn a_service_can_be_a_task_in_the_repositorys_own_runner() {
        let manifest = parse(
            "[services.api]\ntask = \"api#dev\"\nhost = \"api\"\nhealth.http = \"/health\"\n",
        )
        .unwrap();
        assert_eq!(manifest.services[0].task.as_deref(), Some("api#dev"));
        assert_eq!(manifest.services[0].command(), Some("api#dev"));
        assert!(
            manifest.problems().is_empty(),
            "a task is something to start: {:?}",
            manifest.problems()
        );
    }

    #[test]
    fn declaring_two_ways_to_start_one_service_is_refused() {
        let manifest = parse("[services.api]\ntask = \"api#dev\"\nrun = \"pnpm dev\"\n").unwrap();
        assert!(manifest
            .problems()
            .iter()
            .any(|p| p.contains("more than one of run, task, and image")));
    }

    #[test]
    fn stop_semantics_are_declarable_and_validated() {
        let manifest = parse(
            "[services.api]\nrun = \"pnpm dev\"\nstop_signal = \"SIGTERM\"\nstop_grace = \"30s\"\n",
        )
        .unwrap();
        assert_eq!(manifest.services[0].stop_signal.as_deref(), Some("SIGTERM"));
        assert_eq!(
            manifest.services[0].stop_grace,
            Some(std::time::Duration::from_secs(30))
        );
        // A signal that does not exist is caught at parse rather than at the
        // moment someone is trying to stop a service.
        assert!(parse("[services.a]\nrun = \"x\"\nstop_signal = \"SIGBANANA\"\n").is_err());
        assert!(parse("[services.a]\nrun = \"x\"\nstop_grace = \"soon\"\n").is_err());
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

impl Manifest {
    /// What this manifest asks for that is correct but not yet built.
    ///
    /// Deliberately not part of `problems`. "Your manifest is wrong" and "we
    /// cannot do that yet" are different sentences with different audiences:
    /// the first is the author's to fix, the second is ours. Merging them
    /// would also make a perfectly valid manifest report as invalid, which is
    /// a lie about the file.
    ///
    /// Currently empty. It stays because the distinction it draws is the
    /// point, and the next capability gap should land here rather than be
    /// dressed up as a problem with someone's file.
    pub fn unsupported(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A disagreement between what the manifest says a value *is* and how
/// `turbo.json` says it is *treated*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheFinding {
    pub key: String,
    pub message: String,
    /// True when the consequence is a wrong cache *hit* rather than a miss.
    /// A miss is slow; a hit is wrong, and wrong is worse.
    pub silent: bool,
}

impl Manifest {
    /// Where the env catalog and `turbo.json` disagree about the cache.
    ///
    /// Neither file can answer this alone, which is the whole reason to hold
    /// both. `turbo.json` knows a key is hashed but not that it is a secret;
    /// the manifest knows it is a secret but not that turbo hashes it. Only
    /// something holding both can say the sentence that matters.
    pub fn cache_findings(&self, turbo: &crate::turbo::Turbo) -> Vec<CacheFinding> {
        use crate::turbo::covers;

        let hashed = turbo.hashed_patterns();
        let passthrough = turbo.passthrough_patterns();
        let strict = turbo.env_mode.as_deref() != Some("loose");
        let mut findings = Vec::new();

        for rule in &self.env_rules {
            let key = &rule.key;
            let in_hash = covers(&hashed, key);
            let in_passthrough = covers(&passthrough, key);

            if rule.source.is_secret() && in_hash {
                findings.push(CacheFinding {
                    key: key.clone(),
                    message: format!(
                        "{key} is a {} secret but turbo.json hashes it — every machine's \
                         value differs, so this misses cache everywhere, and the value \
                         itself becomes part of a key that travels to your remote cache. \
                         Move it to passThroughEnv.",
                        rule.source.as_str()
                    ),
                    silent: false,
                });
                continue;
            }

            if rule.hashed() && !in_hash {
                let where_it_is = if in_passthrough {
                    "turbo.json only passes it through"
                } else if strict {
                    "turbo.json does not declare it at all, and strict mode will not even \
                     pass it to the task"
                } else {
                    "turbo.json does not declare it at all"
                };
                findings.push(CacheFinding {
                    key: key.clone(),
                    message: format!(
                        "{key} changes behaviour but {where_it_is}, so changing it does not \
                         bust the cache — a build with the old value will be reused and \
                         look green. Add it to env or globalEnv.",
                    ),
                    // The dangerous direction: this one is silent.
                    silent: true,
                });
            }
        }

        findings
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::turbo;

    fn manifest(text: &str) -> Manifest {
        parse(text).unwrap()
    }

    #[test]
    fn a_keychain_secret_in_the_hash_is_reported() {
        let m = manifest("[env.STRIPE_SECRET_KEY]\nfrom = \"keychain\"\n");
        let t = turbo::parse(r#"{"globalEnv": ["STRIPE_SECRET_KEY"]}"#).unwrap();
        let findings = m.cache_findings(&t);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("passThroughEnv"),
            "{:?}",
            findings[0]
        );
        assert!(!findings[0].silent, "a cache miss is loud, just wasteful");
    }

    /// A wildcard is the common way this happens: nobody writes the secret's
    /// name into `globalEnv`, they write `STRIPE_*` and forget what it covers.
    #[test]
    fn a_wildcard_catches_the_secret_too() {
        let m = manifest("[env.STRIPE_SECRET_KEY]\nfrom = \"keychain\"\n");
        let t = turbo::parse(r#"{"globalEnv": ["STRIPE_*"]}"#).unwrap();
        assert_eq!(m.cache_findings(&t).len(), 1);
    }

    #[test]
    fn a_secret_in_passthrough_is_exactly_right() {
        let m = manifest("[env.STRIPE_SECRET_KEY]\nfrom = \"keychain\"\n");
        let t = turbo::parse(r#"{"globalPassThroughEnv": ["STRIPE_*"]}"#).unwrap();
        assert!(m.cache_findings(&t).is_empty());
    }

    /// The worse failure: config that changes behaviour, outside the hash.
    /// It does not cost you a rebuild — it hands you the wrong build.
    #[test]
    fn behaviour_changing_config_outside_the_hash_is_flagged_as_silent() {
        let m = manifest("[env.NEXT_PUBLIC_API_URL]\nfrom = \"literal\"\nhash = true\n");
        let t = turbo::parse(r#"{"globalPassThroughEnv": ["NEXT_PUBLIC_*"]}"#).unwrap();
        let findings = m.cache_findings(&t);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].silent, "a wrong cache hit is the quiet one");
        assert!(findings[0].message.contains("only passes it through"));
    }

    #[test]
    fn strict_mode_is_named_when_a_key_is_declared_nowhere() {
        let m = manifest("[env.API_URL]\nfrom = \"literal\"\nhash = true\n");
        let t = turbo::parse(r#"{"envMode": "strict"}"#).unwrap();
        let findings = m.cache_findings(&t);
        assert!(
            findings[0].message.contains("strict mode"),
            "{:?}",
            findings[0]
        );
    }

    #[test]
    fn a_correctly_hashed_value_is_not_a_finding() {
        let m = manifest("[env.API_URL]\nfrom = \"literal\"\nhash = true\n");
        let t = turbo::parse(r#"{"globalEnv": ["API_URL"]}"#).unwrap();
        assert!(m.cache_findings(&t).is_empty());
    }

    /// The default exists so the common case needs no annotation: a value
    /// from the Keychain is a secret, and a secret is never hashed.
    #[test]
    fn defaults_follow_the_source_when_the_author_says_nothing() {
        let m = manifest("[env.SOME_TOKEN]\nrequire_prefix = \"sk_test_\"\n");
        assert_eq!(m.env_rules[0].source, EnvSource::Keychain);
        assert!(!m.env_rules[0].hashed());

        let m = manifest("[env.PUBLIC_URL]\nfrom = \"literal\"\n");
        assert!(
            m.env_rules[0].hashed(),
            "a literal is config, so it is hashed"
        );
    }

    #[test]
    fn an_unknown_source_is_refused_rather_than_defaulted() {
        assert!(parse("[env.X]\nfrom = \"somewhere\"\n").is_err());
    }
}
