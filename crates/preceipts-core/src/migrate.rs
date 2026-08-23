//! Converting a `procpane.toml` into a `preceipts.toml`, once.
//!
//! The two files describe the same stack in different shapes. `procpane.toml`
//! is a set of *amendments to turbo tasks*, keyed by `pkg#task`;
//! `preceipts.toml` is a *service graph* of named services. Reading both
//! forever would mean the old model never actually leaves — it would just wear
//! a new coat, translated at every boot. Rewriting the file on disk is what
//! retires it.
//!
//! The conversion is deliberately lossless in the direction that matters: if
//! something in the old file has no home in the new schema, that is a gap in
//! the new schema and gets fixed there, not dropped here. `health.start_period`
//! exists in `preceipts.toml` because trip sets it on three services and
//! losing it would silently make their health checks lie.

use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// What a conversion produced, and what a person should look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    /// The `preceipts.toml` to write.
    pub manifest: String,
    /// Things worth a human's attention. Never silent: a conversion that
    /// quietly changed the meaning of a stack would be worse than one that
    /// refused to run.
    pub notes: Vec<String>,
}

/// Read `procpane.toml` at `root` and produce the equivalent manifest.
pub fn from_procpane(root: &Path) -> Result<Option<Conversion>> {
    let path = root.join("procpane.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    convert(&text).map(Some)
}

/// A service name for a task, and where it came from.
///
/// The hostname label is the best name available: it is what a person types
/// into a browser and what the URL will keep saying. Failing that, the package
/// tail — `@trip/db#db:up` is the `db` service to everyone who works on it.
fn service_name(task_id: &str, hostname: Option<&str>) -> String {
    if let Some(host) = hostname {
        if let Some(label) = host.split('.').next() {
            if !label.is_empty() {
                return label.to_string();
            }
        }
    }
    let package = task_id.split_once('#').map(|(p, _)| p).unwrap_or(task_id);
    package
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(task_id)
        .to_string()
}

#[cfg(test)]
mod key_quoting {
    use super::*;

    /// The bug this guards: an unquoted `chart.js` is a dotted key, so the
    /// service lands three tables deep and the manifest parses fine while
    /// meaning something else entirely.
    #[test]
    fn a_package_name_with_a_dot_stays_one_service() {
        let converted = convert("[tasks.\"chart.js#build\"]\nhealthcheck.tcp = 1\n").unwrap();
        let manifest = crate::manifest::parse(&converted.manifest).unwrap();
        assert_eq!(manifest.services.len(), 1, "{}", converted.manifest);
        assert_eq!(manifest.services[0].name, "chart.js");
    }

    /// And a name TOML could not express bare at all must not break the file.
    #[test]
    fn a_name_with_a_quote_or_a_space_still_produces_valid_toml() {
        let converted = convert("[tasks.\"my app#dev\"]\nhealthcheck.tcp = 1\n").unwrap();
        assert!(
            crate::manifest::parse(&converted.manifest).is_ok(),
            "{}",
            converted.manifest
        );
    }
}

pub fn convert(text: &str) -> Result<Conversion> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| {
        Error::Git(git2::Error::from_str(&format!(
            "parsing procpane.toml: {e}"
        )))
    })?;
    let tasks = doc.get("tasks").and_then(|t| t.as_table()).ok_or_else(|| {
        Error::Git(git2::Error::from_str(
            "procpane.toml has no [tasks] section",
        ))
    })?;

    let mut notes = Vec::new();

    // Name every task first: `needs` refers to services by name, so the whole
    // mapping has to exist before any single service can be written.
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    for (task_id, overlay) in tasks {
        let hostname = overlay.get("hostname").and_then(|v| v.as_str());
        let base = service_name(task_id, hostname);
        let count = used.entry(base.clone()).or_insert(0);
        *count += 1;
        let name = if *count == 1 {
            base
        } else {
            // Two tasks wanting one name is rare and always worth reading
            // about, rather than being resolved silently.
            let disambiguated = format!("{base}-{count}");
            notes.push(format!(
                "two tasks both wanted the service name \"{base}\"; \
                 \"{task_id}\" became \"{disambiguated}\""
            ));
            disambiguated
        };
        names.insert(task_id.clone(), name);
    }

    let mut out = String::from(
        "# Converted from procpane.toml by `preceipts migrate`.\n\
         #\n\
         # Services, not task overlays: each entry below is a thing with an\n\
         # address and a health check, and `preceipts up` boots them in order.\n",
    );

    for (task_id, overlay) in tasks {
        let name = &names[task_id];
        // The key is quoted like every value. An unquoted `chart.js` would be
        // a *dotted* key — valid TOML that silently means `services.chart.js`,
        // three tables deep, rather than one service. npm names contain dots
        // often enough that this is a when, not an if.
        out.push_str(&format!("\n[services.{}]\n", toml_string(name)));
        out.push_str(&format!("task = {}\n", toml_string(task_id)));

        if let Some(hostname) = overlay.get("hostname").and_then(|v| v.as_str()) {
            // A label, not a domain: the fabric composes the rest, and the
            // old fully-qualified name embedded a TLD that no longer exists.
            let label = hostname.split('.').next().unwrap_or(hostname);
            out.push_str(&format!("host = {}\n", toml_string(label)));
            if hostname != label {
                notes.push(format!(
                    "\"{task_id}\" declared the full hostname \"{hostname}\"; it is now the \
                     label \"{label}\" and the fabric composes \
                     <label>.<workspace>.<project>.localhost"
                ));
            }
        }

        if let Some(health) = overlay.get("healthcheck").and_then(|v| v.as_table()) {
            if let Some(port) = health.get("tcp").and_then(|v| v.as_integer()) {
                out.push_str(&format!("health.tcp = {port}\n"));
            } else if let Some(path) = health.get("http").and_then(|v| v.as_str()) {
                out.push_str(&format!("health.http = {}\n", toml_string(path)));
            } else if let Some(pattern) = health.get("log").and_then(|v| v.as_str()) {
                out.push_str(&format!("health.log = {}\n", toml_string(pattern)));
            }
            for key in ["start_period", "interval", "timeout"] {
                if let Some(value) = health.get(key).and_then(|v| v.as_str()) {
                    out.push_str(&format!("health.{key} = {}\n", toml_string(value)));
                }
            }
            if health.get("exit").is_some() {
                notes.push(format!(
                    "\"{task_id}\" used healthcheck.exit, which has no equivalent — a \
                     one-shot task is ready when it completes"
                ));
            }
        }

        if let Some(depends) = overlay.get("depends_on").and_then(|v| v.as_table()) {
            let mut needs: Vec<String> = Vec::new();
            for (dep_id, condition) in depends {
                match names.get(dep_id) {
                    Some(dep_name) => needs.push(dep_name.clone()),
                    None => notes.push(format!(
                        "\"{task_id}\" depends on \"{dep_id}\", which is not declared in this \
                         file — the dependency was dropped"
                    )),
                }
                if condition.as_str() == Some("started") {
                    notes.push(format!(
                        "\"{task_id}\" waited only for \"{dep_id}\" to *start*; it now waits \
                         for it to be healthy, which is stricter and almost certainly what \
                         you wanted"
                    ));
                }
            }
            if !needs.is_empty() {
                out.push_str(&format!("needs = {}\n", toml_array(&needs)));
            }
        }

        if let Some(env) = overlay.get("env_from").and_then(|v| v.as_array()) {
            let keys: Vec<String> = env
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if !keys.is_empty() {
                out.push_str(&format!("env = {}\n", toml_array(&keys)));
            }
        }

        if let Some(signal) = overlay.get("stop_signal").and_then(|v| v.as_str()) {
            out.push_str(&format!("stop_signal = {}\n", toml_string(signal)));
        }
        if let Some(grace) = overlay.get("stop_grace_period").and_then(|v| v.as_str()) {
            out.push_str(&format!("stop_grace = {}\n", toml_string(grace)));
        }
        if overlay.get("profiles").and_then(|v| v.as_array()).is_some() {
            notes.push(format!(
                "\"{task_id}\" declared profiles, which have no equivalent yet — use \
                 `preceipts up <service>` to start a subset"
            ));
        }
    }

    Ok(Conversion {
        manifest: out,
        notes,
    })
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toml_array(values: &[String]) -> String {
    if values.len() == 1 {
        return format!("[{}]", toml_string(&values[0]));
    }
    let mut out = String::from("[\n");
    for value in values {
        out.push_str(&format!("  {},\n", toml_string(value)));
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;

    #[test]
    fn a_task_becomes_a_service_named_for_its_hostname() {
        let converted = convert(
            "[tasks.\"@trip/api#dev\"]\nhostname = \"api.trip.test\"\nhealthcheck.log = \"ready\"\n",
        )
        .unwrap();
        let manifest =
            manifest::parse(&converted.manifest).expect("the output is a valid manifest");
        assert_eq!(manifest.services[0].name, "api");
        assert_eq!(manifest.services[0].task.as_deref(), Some("@trip/api#dev"));
        assert_eq!(
            manifest.services[0].host.as_deref(),
            Some("api"),
            "a label, not a domain"
        );
        assert!(
            converted.notes.iter().any(|n| n.contains("api.trip.test")),
            "the hostname change is not silent: {:?}",
            converted.notes
        );
    }

    #[test]
    fn a_task_without_a_hostname_is_named_for_its_package() {
        let converted = convert("[tasks.\"@trip/db#db:up\"]\nhealthcheck.tcp = 5432\n").unwrap();
        let manifest = manifest::parse(&converted.manifest).unwrap();
        assert_eq!(manifest.services[0].name, "db");
        assert_eq!(manifest.services[0].health, manifest::Health::Tcp(5432));
    }

    /// The field that made the schema grow: losing it would silently make
    /// three of trip's health checks lie about readiness.
    #[test]
    fn health_timing_survives() {
        let converted = convert(
            "[tasks.\"a#dev\"]\nhealthcheck.log = \"ready\"\nhealthcheck.start_period = \"5s\"\n",
        )
        .unwrap();
        let manifest = manifest::parse(&converted.manifest).unwrap();
        assert_eq!(
            manifest.services[0].health_start_period,
            Some(std::time::Duration::from_secs(5))
        );
    }

    #[test]
    fn dependencies_become_service_names() {
        let converted = convert(
            "[tasks.\"@t/db#up\"]\nhealthcheck.tcp = 5432\n\n\
             [tasks.\"@t/api#dev\"]\nhostname = \"api.t.test\"\n\
             healthcheck.log = \"ready\"\n\
             depends_on.\"@t/db#up\" = \"healthy\"\n",
        )
        .unwrap();
        let manifest = manifest::parse(&converted.manifest).unwrap();
        let api = manifest.services.iter().find(|s| s.name == "api").unwrap();
        assert_eq!(api.needs, vec!["db"], "by name, not by task id");
        assert!(manifest.problems().is_empty(), "{:?}", manifest.problems());
    }

    /// `started` is weaker than `healthy`, and the new schema has only the
    /// stricter one. Changing the meaning of someone's stack is allowed;
    /// doing it without saying so is not.
    #[test]
    fn a_weaker_dependency_condition_is_reported() {
        let converted = convert(
            "[tasks.\"a#x\"]\nhealthcheck.tcp = 1\n\n\
             [tasks.\"b#y\"]\ndepends_on.\"a#x\" = \"started\"\n",
        )
        .unwrap();
        assert!(
            converted.notes.iter().any(|n| n.contains("waits")),
            "{:?}",
            converted.notes
        );
    }

    #[test]
    fn env_allowlists_carry_over_per_service() {
        let converted = convert("[tasks.\"a#x\"]\nenv_from = [\"SECRET\", \"ENV\"]\n").unwrap();
        let manifest = manifest::parse(&converted.manifest).unwrap();
        assert_eq!(manifest.services[0].env, vec!["SECRET", "ENV"]);
    }

    #[test]
    fn two_tasks_wanting_one_name_are_disambiguated_out_loud() {
        let converted = convert(
            "[tasks.\"@a/web#dev\"]\nhostname = \"web.one.test\"\n\n\
             [tasks.\"@b/web#dev\"]\nhostname = \"web.two.test\"\n",
        )
        .unwrap();
        let manifest = manifest::parse(&converted.manifest).unwrap();
        assert_eq!(manifest.services.len(), 2);
        assert!(converted
            .notes
            .iter()
            .any(|n| n.contains("both wanted the service name")));
    }

    #[test]
    fn a_file_with_no_tasks_is_an_honest_error() {
        assert!(convert("# just a comment\n").is_err());
    }
}
