//! The manifest, in the shape the daemon addresses things by.
//!
//! `preceipts.toml` names services. The daemon addresses processes as
//! `pkg#task`, because a monorepo's runner does and a service may *be* one of
//! its tasks. This module is the join between the two, and it is the only
//! place that knows both.
//!
//! What it cannot invent is a task id. A service declared with `run = "…"` is
//! a command, not a task, so it gets a synthetic id under a reserved package
//! name — visible in `preceipts services`, and unambiguous against anything
//! turbo produces.

use crate::services::{DependsOnCondition, Healthcheck, ServiceFacts, Services, StopSignal};
use preceipts_core::manifest::{Health, Manifest, Service};
use std::collections::BTreeMap;

/// The package name synthetic ids live under.
///
/// A real turbo package could in principle be called this; nothing stops it.
/// The alternative — deriving a name and hoping — collides quietly, while this
/// collides visibly and only for someone who named a package `preceipts`.
pub const SYNTHETIC_PACKAGE: &str = "preceipts";

/// The task id the daemon will know a service by.
pub fn task_id(service: &Service) -> String {
    match &service.task {
        Some(task) => task.clone(),
        None => format!("{SYNTHETIC_PACKAGE}#{}", service.name),
    }
}

/// Everything the daemon needs to know about a project's services.
pub fn to_services(manifest: &Manifest) -> Services {
    let mut tasks: BTreeMap<String, ServiceFacts> = BTreeMap::new();

    // `needs` names services; `depends_on` names task ids. The
    // lookup has to go through the manifest, because a service's id is not
    // derivable from its name once `task` is in play.
    let id_of: BTreeMap<&str, String> = manifest
        .services
        .iter()
        .map(|service| (service.name.as_str(), task_id(service)))
        .collect();

    for service in &manifest.services {
        let depends_on = service
            .needs
            .iter()
            .filter_map(|need| id_of.get(need.as_str()).cloned())
            // Healthy rather than started: the whole reason trip's stack boots
            // reliably is that a dependent waits for readiness, not for a pid.
            .map(|id| (id, DependsOnCondition::Healthy))
            .collect();

        let healthcheck = match &service.health {
            Health::None => None,
            Health::Tcp(port) => Some(Healthcheck {
                tcp: Some(*port),
                http: None,
                log: None,
                exit: None,
                interval: None,
                timeout: None,
                start_period: None,
            }),
            Health::Http(path) => Some(Healthcheck {
                tcp: None,
                http: Some(path.clone()),
                log: None,
                exit: None,
                interval: None,
                timeout: None,
                start_period: None,
            }),
            Health::Log(pattern) => Some(Healthcheck {
                tcp: None,
                http: None,
                log: Some(pattern.clone()),
                exit: None,
                interval: None,
                timeout: None,
                start_period: None,
            }),
        };

        tasks.insert(
            task_id(service),
            ServiceFacts {
                hostname: service.host.clone(),
                healthcheck,
                depends_on,
                profiles: Vec::new(),
                stop_signal: service.stop_signal.as_deref().and_then(parse_signal),
                stop_grace_period: service.stop_grace,
                // `@group` references are expanded by the manifest's own env
                // resolution; what reaches here is the flat list of keys this
                // service may see.
                env_from: expand_env(manifest, service),
            },
        );
    }

    Services { tasks }
}

fn parse_signal(text: &str) -> Option<StopSignal> {
    Some(match text {
        "SIGINT" => StopSignal::Int,
        "SIGTERM" => StopSignal::Term,
        "SIGHUP" => StopSignal::Hup,
        "SIGQUIT" => StopSignal::Quit,
        _ => return None,
    })
}

/// Resolve a service's `env` list, expanding `@group` references.
///
/// Per-service by design: a stray `postinstall` in one package must not be
/// able to read another's Stripe key, which is only true if the allowlist is
/// per service rather than per project.
fn expand_env(manifest: &Manifest, service: &Service) -> Vec<String> {
    let mut keys = Vec::new();
    for entry in &service.env {
        match entry.strip_prefix('@') {
            None => keys.push(entry.clone()),
            Some(group) => {
                if let Some(found) = manifest.env_groups.iter().find(|g| g.name == group) {
                    keys.extend(found.keys.iter().cloned());
                }
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use preceipts_core::manifest::parse;

    #[test]
    fn a_service_bound_to_a_turbo_task_keeps_that_id() {
        let manifest = parse("[services.api]\ntask = \"api#dev\"\nhost = \"api\"\n").unwrap();
        let services = to_services(&manifest);
        assert!(services.tasks.contains_key("api#dev"));
        assert_eq!(
            services.tasks["api#dev"].hostname.as_deref(),
            Some("api"),
            "the hostname is a label; the fabric composes the rest"
        );
    }

    /// A plain command is not a turbo task, so it needs an id the daemon can
    /// address it by — and one that cannot be mistaken for a real package.
    #[test]
    fn a_plain_command_gets_a_synthetic_id() {
        let manifest = parse("[services.db]\nimage = \"postgres:17\"\n").unwrap();
        let services = to_services(&manifest);
        assert!(services.tasks.contains_key("preceipts#db"));
    }

    #[test]
    fn needs_becomes_depends_on_healthy_across_both_kinds_of_id() {
        let manifest = parse(
            "[services.db]\nimage = \"postgres:17\"\nhealth.tcp = 5432\n\n\
             [services.api]\ntask = \"api#dev\"\nneeds = [\"db\"]\n",
        )
        .unwrap();
        let services = to_services(&manifest);
        let api = &services.tasks["api#dev"];
        assert_eq!(
            api.depends_on.get("preceipts#db"),
            Some(&DependsOnCondition::Healthy),
            "a dependent waits for readiness, not for a pid: {:?}",
            api.depends_on
        );
    }

    #[test]
    fn each_health_kind_survives_the_translation() {
        let manifest = parse(
            "[services.a]\nrun = \"a\"\nhealth.tcp = 5432\n\n\
             [services.b]\nrun = \"b\"\nhealth.http = \"/health\"\n\n\
             [services.c]\nrun = \"c\"\nhealth.log = \"ready\"\n\n\
             [services.d]\nrun = \"d\"\n",
        )
        .unwrap();
        let services = to_services(&manifest);
        assert_eq!(
            services.tasks["preceipts#a"]
                .healthcheck
                .as_ref()
                .unwrap()
                .tcp,
            Some(5432)
        );
        assert_eq!(
            services.tasks["preceipts#b"]
                .healthcheck
                .as_ref()
                .unwrap()
                .http
                .as_deref(),
            Some("/health")
        );
        assert_eq!(
            services.tasks["preceipts#c"]
                .healthcheck
                .as_ref()
                .unwrap()
                .log
                .as_deref(),
            Some("ready")
        );
        assert!(
            services.tasks["preceipts#d"].healthcheck.is_none(),
            "nothing declared stays nothing declared"
        );
    }

    #[test]
    fn env_groups_expand_and_stay_per_service() {
        let manifest = parse(
            "[env.shared]\nkeys = [\"SENTRY_DSN\", \"ENV\"]\n\n\
             [services.api]\nrun = \"a\"\nenv = [\"@shared\", \"STRIPE_SECRET_KEY\"]\n\n\
             [services.worker]\nrun = \"w\"\nenv = [\"ENV\"]\n",
        )
        .unwrap();
        let services = to_services(&manifest);
        assert_eq!(
            services.tasks["preceipts#api"].env_from,
            vec!["ENV", "SENTRY_DSN", "STRIPE_SECRET_KEY"]
        );
        assert_eq!(
            services.tasks["preceipts#worker"].env_from,
            vec!["ENV"],
            "the worker cannot see the api's Stripe key"
        );
    }

    #[test]
    fn stop_semantics_survive_the_translation() {
        let manifest =
            parse("[services.api]\nrun = \"a\"\nstop_signal = \"SIGTERM\"\nstop_grace = \"30s\"\n")
                .unwrap();
        let service = &to_services(&manifest).tasks["preceipts#api"];
        assert_eq!(service.stop_signal, Some(StopSignal::Term));
        assert_eq!(
            service.stop_grace_period,
            Some(std::time::Duration::from_secs(30))
        );
    }

    /// A `needs` naming a service that does not exist is a manifest problem,
    /// reported by `doctor`. The bridge drops it rather than inventing a task
    /// id the daemon would then wait on forever.
    #[test]
    fn a_dangling_need_is_dropped_rather_than_invented() {
        let manifest = parse("[services.api]\nrun = \"a\"\nneeds = [\"ghost\"]\n").unwrap();
        assert!(to_services(&manifest).tasks["preceipts#api"]
            .depends_on
            .is_empty());
        assert!(
            !manifest.problems().is_empty(),
            "and doctor still says so rather than the daemon hanging"
        );
    }
}
