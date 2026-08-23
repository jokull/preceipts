//! The design's own acceptance test, set in docs/direction-2026-08.md:
//!
//! > trip's sandbox should be expressible as one `preceipts.toml` plus its
//! > existing scripts [...] If it can't, the schema is wrong, and trip is the
//! > benchmark to check against before writing the parser.
//!
//! So this checks it. The fixture is built from trip's real `procpane.toml`
//! and `packages/sandbox/README.md` — the kitchen-sink end of the range the
//! schema has to reach without becoming a program.

use preceipts_core::manifest::{self, Fidelity, Health, Runtime};

const TRIP: &str = include_str!("fixtures/trip.toml");

#[test]
fn trips_sandbox_is_expressible_and_valid() {
    let manifest = manifest::parse(TRIP).expect("trip's manifest parses");
    let problems = manifest.problems();
    assert!(
        problems.is_empty(),
        "trip's manifest has problems: {problems:#?}"
    );
}

#[test]
fn the_stack_boots_in_a_workable_order() {
    let manifest = manifest::parse(TRIP).unwrap();
    let order: Vec<&str> = manifest
        .boot_order()
        .unwrap()
        .iter()
        .map(|s| s.name.as_str())
        .collect();

    let position = |name: &str| order.iter().position(|n| *n == name).unwrap();
    assert!(position("db") < position("api"), "postgres before the api");
    assert!(
        position("stripe-webhook") < position("api"),
        "the webhook listener mints a secret the api needs at boot"
    );
    assert!(
        position("api") < position("next"),
        "SSR must not race the api"
    );
}

#[test]
fn the_stack_is_mixed_native_and_container() {
    let manifest = manifest::parse(TRIP).unwrap();
    assert_eq!(
        manifest.service("db").unwrap().runtime,
        Runtime::Container,
        "containerise the stateful thing"
    );
    assert_eq!(
        manifest.service("next").unwrap().runtime,
        Runtime::Native,
        "run the JS natively, where iteration speed lives"
    );
}

#[test]
fn live_credentials_are_refused_by_assertion_not_convention() {
    let manifest = manifest::parse(TRIP).unwrap();
    let rule = manifest
        .env_rules
        .iter()
        .find(|r| r.key == "STRIPE_SECRET_KEY")
        .expect("stripe key is governed");
    assert_eq!(rule.require_prefix.as_deref(), Some("sk_test_"));
}

#[test]
fn env_groups_collapse_the_repetition_that_bloated_the_original() {
    let manifest = manifest::parse(TRIP).unwrap();
    // trip's procpane.toml repeated ~20 keys across three services. Here each
    // service names a couple of groups.
    for name in ["api", "next", "admin"] {
        let service = manifest.service(name).unwrap();
        assert!(
            service.env.len() <= 4,
            "{name} still lists {} env entries",
            service.env.len()
        );
        assert!(service.env.iter().any(|e| e.starts_with('@')));
    }
}

#[test]
fn hostnames_are_labels_so_the_fabric_composes_them() {
    let manifest = manifest::parse(TRIP).unwrap();
    for (service, label) in [("api", "api"), ("next", "web"), ("admin", "admin")] {
        assert_eq!(
            manifest.service(service).unwrap().host.as_deref(),
            Some(label)
        );
    }
    // No fully-qualified domain anywhere: that is what made trip's hostnames
    // wrong once workspaces existed.
    assert!(
        !TRIP.contains(".test") && !TRIP.contains("trip.local"),
        "a manifest should never spell a domain"
    );
}

#[test]
fn side_effects_are_caught_in_named_drains() {
    let manifest = manifest::parse(TRIP).unwrap();
    let kinds: Vec<&str> = manifest.drains.iter().map(|d| d.name.as_str()).collect();
    for expected in ["email", "analytics", "errors", "external-requests"] {
        assert!(
            kinds.contains(&expected),
            "missing drain {expected}: {kinds:?}"
        );
    }
}

#[test]
fn the_readiness_gate_is_a_claim_above_health() {
    let manifest = manifest::parse(TRIP).unwrap();
    assert!(manifest.ready.contains(&"api".to_string()));
    // stripe-webhook is healthy-gated for the api but is not something browser
    // work waits on — the two lists are genuinely different.
    assert!(!manifest.ready.contains(&"stripe-webhook".to_string()));
}

#[test]
fn seed_radii_compose_the_way_trip_uses_them() {
    let manifest = manifest::parse(TRIP).unwrap();
    let small = manifest.resolve_radius("s").unwrap();
    let large = manifest.resolve_radius("l").unwrap();
    assert!(small.len() < large.len());
    for bundle in &small {
        assert!(large.contains(bundle), "l is a superset of s");
    }
    assert!(large.contains(&"checkout".to_string()));
    // Every bundle a radius names must exist.
    for name in large {
        assert!(
            manifest.bundles.iter().any(|b| b.name == name),
            "radius names undefined bundle {name}"
        );
    }
}

#[test]
fn the_domain_specific_half_is_verbs_we_do_not_model() {
    let manifest = manifest::parse(TRIP).unwrap();
    for verb in [
        "auth",
        "purchase-smoke",
        "provider-contracts",
        "emails",
        "posthog",
    ] {
        assert!(manifest.action(verb).is_some(), "missing action {verb}");
    }
    // Arguments are declared, so a CLI command and an MCP tool can both be
    // generated without knowing what a "seeded actor" is.
    assert_eq!(manifest.action("auth").unwrap().args, ["actor"]);
    assert_eq!(manifest.action("posthog").unwrap().args, ["events"]);
}

#[test]
fn fidelity_is_stated_so_nothing_can_over_claim() {
    let manifest = manifest::parse(TRIP).unwrap();
    assert_eq!(
        manifest.service("stripe-webhook").unwrap().fidelity,
        Fidelity::LocalSimulated,
        "Stripe.js is shimmed: a browse is route proof, not payment proof"
    );
    assert_eq!(
        manifest.service("urgentry").unwrap().fidelity,
        Fidelity::LocalReal
    );
    assert!(
        manifest.mocks.iter().all(|m| m.record),
        "mocked traffic is recorded"
    );
}

#[test]
fn health_checks_are_declared_for_everything_that_publishes_a_url() {
    let manifest = manifest::parse(TRIP).unwrap();
    for service in &manifest.services {
        if service.host.is_some() {
            assert_ne!(
                service.health,
                Health::None,
                "{} publishes a URL with nothing to wait on",
                service.name
            );
        }
    }
}

/// trip's real `turbo.json`, reduced to its env declarations.
///
/// A synthetic fixture would not have caught the thing that actually breaks a
/// JSON parser here: turbo names its root tasks `//#format`, which is
/// indistinguishable from a comment to anything that is not string-aware.
const TRIP_TURBO: &str = include_str!("fixtures/trip-turbo.json");

#[test]
fn trips_turbo_json_reads_including_its_comment_shaped_task_names() {
    let turbo = preceipts_core::turbo::parse(TRIP_TURBO).expect("trip's turbo.json parses");
    let passthrough = turbo.passthrough_patterns();
    assert!(
        passthrough.contains("DATABASE_*"),
        "the wildcards trip actually uses survive"
    );
    assert!(
        turbo.hashed_patterns().is_empty(),
        "trip hashes nothing globally, which is a real and correct state"
    );
}

/// The sentence no other tool can say, checked against a real repository's
/// configuration rather than a fixture written to make it true.
#[test]
fn a_secret_trip_passes_through_is_correct_and_reported_as_such() {
    let turbo = preceipts_core::turbo::parse(TRIP_TURBO).unwrap();
    // trip passes DATABASE_* through, never hashes it — exactly right.
    let manifest = preceipts_core::manifest::parse("[env.DATABASE_URL]\nfrom = \"keychain\"\n")
        .expect("a manifest");
    assert!(
        manifest.cache_findings(&turbo).is_empty(),
        "a secret in passThroughEnv is the correct arrangement"
    );

    // And config that changes behaviour, declared nowhere, is the silent bug.
    let manifest =
        preceipts_core::manifest::parse("[env.NEXT_PUBLIC_SITE_URL]\nfrom = \"literal\"\n")
            .expect("a manifest");
    let findings = manifest.cache_findings(&turbo);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0].silent,
        "a wrong cache hit is the one that costs you"
    );
}
