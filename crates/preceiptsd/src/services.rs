use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// The daemon's view of a project's services, keyed by task id.
///
/// Built from `preceipts.toml` by `bridge` and never parsed from a file —
/// which is why there is no `serde` here at all. It was a parsed sidecar
/// once; keeping the deserializer would have kept the old file readable by
/// accident.
#[derive(Debug, Clone, Default)]
pub struct Services {
    pub tasks: BTreeMap<String, ServiceFacts>,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceFacts {
    pub hostname: Option<String>,
    pub healthcheck: Option<Healthcheck>,
    pub depends_on: BTreeMap<String, DependsOnCondition>,
    pub profiles: Vec<String>,
    pub stop_signal: Option<StopSignal>,
    pub stop_grace_period: Option<Duration>,
    pub env_from: Vec<String>,
}

/// Healthcheck — at most one kind per task in this MVP.
/// (Compose allows lists; we keep it flat for clarity.)
#[derive(Debug, Clone)]
pub struct Healthcheck {
    pub tcp: Option<u16>,
    pub http: Option<String>,
    pub log: Option<String>,
    pub exit: Option<i32>,
    /// Seconds between probes (default 1s)
    pub interval: Option<Duration>,
    /// Per-probe timeout (default 2s)
    pub timeout: Option<Duration>,
    /// Grace period before first probe runs (default 0)
    pub start_period: Option<Duration>,
}

impl Healthcheck {
    pub fn interval(&self) -> Duration {
        self.interval.unwrap_or(Duration::from_secs(1))
    }
    pub fn timeout(&self) -> Duration {
        self.timeout.unwrap_or(Duration::from_secs(2))
    }
    pub fn start_period(&self) -> Duration {
        self.start_period.unwrap_or(Duration::from_millis(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependsOnCondition {
    /// Process has been spawned.
    Started,
    /// Healthcheck has reported healthy.
    Healthy,
    /// One-shot task ran to completion successfully.
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopSignal {
    Int,
    Term,
    Hup,
    Quit,
}

impl StopSignal {
    pub fn as_libc(&self) -> i32 {
        match self {
            StopSignal::Int => libc::SIGINT,
            StopSignal::Term => libc::SIGTERM,
            StopSignal::Hup => libc::SIGHUP,
            StopSignal::Quit => libc::SIGQUIT,
        }
    }
}

impl Services {
    /// Load the project's service declarations from `preceipts.toml`.
    ///
    /// One file. A `procpane.toml` is *reported*, never read: it describes a
    /// different shape — amendments to turbo tasks rather than a service graph
    /// — and quietly translating it at every boot would mean the old model
    /// never actually leaves, it just wears a new coat. `preceipts migrate`
    /// rewrites it once and then the question is settled.
    pub fn load(root: &Path) -> Result<Self> {
        if let Some(manifest) = preceipts_core::manifest::load(root)
            .map_err(|e| anyhow::anyhow!("reading preceipts.toml: {e}"))?
        {
            return Ok(crate::bridge::to_services(&manifest));
        }
        if root.join("procpane.toml").is_file() {
            return Err(anyhow::anyhow!(
                "this project still has a procpane.toml and no preceipts.toml — \
                 run `preceipts migrate` to convert it (`--dry-run` first if you \
                 want to read the result before it is written)"
            ));
        }
        Ok(Self::default())
    }

    /// Look up a service by task id. Tries canonical `pkg#task` first,
    /// then short `shortpkg#task` if provided.
    pub fn service(&self, canonical: &str, short: Option<&str>) -> Option<&ServiceFacts> {
        if let Some(o) = self.tasks.get(canonical) {
            return Some(o);
        }
        if let Some(s) = short {
            return self.tasks.get(s);
        }
        None
    }
}

impl ServiceFacts {
    /// SIGINT by default: dev servers overwhelmingly treat it as "shut down
    /// cleanly", and SIGTERM is the one that leaves half-written caches.
    pub fn stop_signal(&self) -> i32 {
        self.stop_signal
            .map(|s| s.as_libc())
            .unwrap_or(libc::SIGINT)
    }

    pub fn stop_grace(&self) -> Duration {
        self.stop_grace_period.unwrap_or(Duration::from_secs(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These used to be parser tests. There is no parser: `preceipts.toml` is
    /// read by the manifest and turned into these types by `bridge`. What is
    /// left worth pinning is the defaults, because they are what a project
    /// gets when it declares nothing — which is most projects.
    #[test]
    fn stopping_defaults_to_sigint_after_five_seconds() {
        let facts = ServiceFacts::default();
        assert_eq!(
            facts.stop_signal(),
            libc::SIGINT,
            "dev servers treat SIGINT as \"shut down cleanly\"; SIGTERM is the \
             one that leaves half-written caches"
        );
        assert_eq!(facts.stop_grace(), Duration::from_secs(5));
    }

    #[test]
    fn a_declared_signal_and_grace_win() {
        let facts = ServiceFacts {
            stop_signal: Some(StopSignal::Term),
            stop_grace_period: Some(Duration::from_secs(30)),
            ..Default::default()
        };
        assert_eq!(facts.stop_signal(), libc::SIGTERM);
        assert_eq!(facts.stop_grace(), Duration::from_secs(30));
    }

    #[test]
    fn health_probe_timing_has_workable_defaults() {
        let health = Healthcheck {
            tcp: None,
            http: None,
            log: Some("ready".to_string()),
            exit: None,
            interval: None,
            timeout: None,
            start_period: None,
        };
        assert_eq!(health.interval(), Duration::from_secs(1));
        assert_eq!(health.timeout(), Duration::from_secs(2));
        assert_eq!(
            health.start_period(),
            Duration::ZERO,
            "probing immediately is right until a service says otherwise"
        );
    }

    #[test]
    fn a_service_is_found_by_canonical_or_short_id() {
        let mut services = Services::default();
        services.tasks.insert(
            "@trip/api#dev".to_string(),
            ServiceFacts {
                hostname: Some("api".to_string()),
                ..Default::default()
            },
        );
        assert!(services.service("@trip/api#dev", None).is_some());
        assert!(services.service("nope", Some("@trip/api#dev")).is_some());
        assert!(services.service("nope", Some("also-nope")).is_none());
    }
}
