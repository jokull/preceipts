//! Starting a service that declares an `image`, by delegating.
//!
//! The direction doc is emphatic that we do not write a VMM, and the reason
//! holds up: a mini-OrbStack is a hypervisor *plus* OCI image handling,
//! filesystem sharing, networking and lifecycle management — years of work,
//! and the least differentiated code in the product. Everyone shipping agent
//! sandboxes has isolation. Nobody has `api.fix-checkout.trip.localhost` with
//! an HTTP transcript and a tree-keyed receipt.
//!
//! So this is an adapter over a CLI, written against what is actually
//! installed rather than what the docs describe.
//!
//! ## The failure that matters
//!
//! A binary on `PATH` whose backend is stopped is the case worth handling
//! carefully, because it does not fail — it *hangs*. OrbStack starts its VM
//! when something touches the Docker socket, so `docker info` against a
//! stopped backend can block for seconds while a virtual machine boots. An
//! adapter that treats "the binary exists" as "the runtime is up" turns that
//! into a service that never comes healthy and a person wondering why.
//!
//! Hence the order: is the CLI there, is the backend up according to the
//! *supervisor*, and only then talk to the socket.

use anyhow::{anyhow, Result};
use std::process::Command;
use std::time::Duration;

/// How long any single probe may take.
///
/// Short on purpose: every call here is a question about local state, and a
/// question about local state that takes longer than this is not going to be
/// answered by waiting.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// Installed, backend not running. The message says how to fix it.
    NotRunning(String),
    /// Nothing to delegate to.
    Missing(String),
}

impl Availability {
    pub fn is_ready(&self) -> bool {
        matches!(self, Availability::Ready)
    }

    pub fn explain(&self) -> String {
        match self {
            Availability::Ready => "container runtime ready".to_string(),
            Availability::NotRunning(why) | Availability::Missing(why) => why.clone(),
        }
    }
}

fn on_path(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                let candidate = dir.join(binary);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

/// Run a command with a deadline, so a booting VM cannot hold us.
fn bounded(mut command: Command) -> Result<std::process::Output> {
    use std::io::Read;
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_end(&mut stdout);
            }
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_end(&mut stderr);
            }
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("timed out after {PROBE_TIMEOUT:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Can we start containers right now?
pub fn availability() -> Availability {
    if !on_path("docker") {
        return Availability::Missing(
            "no container runtime found — install OrbStack, Docker, or Apple's `container`, \
             or give the service a `run` command instead of an `image`"
                .to_string(),
        );
    }

    // Ask the supervisor before the socket. `orbctl status` is a documented
    // contract — 0 running, 1 stopped, 2 starting — and answers without
    // touching the socket that would otherwise boot a VM under us.
    if on_path("orbctl") {
        let mut command = Command::new("orbctl");
        command.arg("status");
        match bounded(command) {
            Ok(output) => match output.status.code() {
                Some(0) => return Availability::Ready,
                Some(2) => {
                    return Availability::NotRunning(
                        "OrbStack is still starting — try again in a moment".to_string(),
                    )
                }
                _ => {
                    return Availability::NotRunning(
                        "OrbStack is installed but not running — start it, or give the \
                         service a `run` command instead of an `image`"
                            .to_string(),
                    )
                }
            },
            Err(e) => return Availability::NotRunning(format!("asking OrbStack its status: {e}")),
        }
    }

    // No supervisor to ask, so the socket is the only source. Bounded,
    // because this is exactly where a stopped backend hangs.
    let mut command = Command::new("docker");
    command.args(["version", "--format", "{{.Server.Version}}"]);
    match bounded(command) {
        Ok(output) if output.status.success() => Availability::Ready,
        Ok(_) => Availability::NotRunning(
            "a docker CLI is installed but its daemon is not answering".to_string(),
        ),
        Err(e) => Availability::NotRunning(format!("probing the docker daemon: {e}")),
    }
}

/// What a container is called for a service in a workspace.
///
/// Named rather than anonymous so a crashed daemon's container can be found
/// and cleaned up, and namespaced by workspace so two worktrees do not fight
/// over one name — the same reason their ports and hostnames are namespaced.
pub fn container_name(workspace_key: &str, service: &str) -> String {
    let slug: String = workspace_key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("preceipts-{slug}-{service}")
}

/// The container name for a service, resolved from the environment.
///
/// The workspace key is not known where the command string is built — it is a
/// property of where the daemon is running, not of the manifest — so the shell
/// interpolates it at spawn time from a variable the daemon injects. That
/// keeps one name in one place rather than two that must agree.
pub fn container_name_env(service: &str) -> String {
    format!("preceipts-${{PRECEIPTS_WORKSPACE_KEY}}-{service}")
}

/// Is this container running, absent, or merely stopped?
///
/// Three states, deliberately not two. `docker ps --filter name=` matches by
/// *substring*, so `db` finds `xj-greenfield-db-postgres-1`; the filter is
/// anchored here for that reason. And "absent" and "stopped" have to be told
/// apart, because one means start it and the other means remove it first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Stopped,
    Absent,
}

pub fn state(name: &str) -> Result<State> {
    let mut command = Command::new("docker");
    command.args([
        "ps",
        "-a",
        "--filter",
        &format!("name=^{name}$"),
        "--format",
        "{{.State}}",
    ]);
    let output = bounded(command)?;
    if !output.status.success() {
        return Err(anyhow!(
            "docker ps: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    match text.trim() {
        "" => Ok(State::Absent),
        "running" => Ok(State::Running),
        _ => Ok(State::Stopped),
    }
}

/// Start a service's container, replacing a stopped one of the same name.
pub fn start(
    name: &str,
    image: &str,
    host_port: u16,
    container_port: u16,
    env: &[(String, String)],
) -> Result<()> {
    // A stopped container with our name holds the name and nothing else.
    // Removing it is not destructive in the way it sounds: the workspace's
    // data lives in a volume or in the image, never in a dev container we
    // named ourselves.
    if state(name)? != State::Absent {
        let mut remove = Command::new("docker");
        remove.args(["rm", "-f", "-v", name]);
        let _ = bounded(remove);
    }

    let mut command = Command::new("docker");
    command.args(["run", "-d", "--name", name]);
    command.args(["-p", &format!("127.0.0.1:{host_port}:{container_port}")]);
    for (key, value) in env {
        command.args(["-e", &format!("{key}={value}")]);
    }
    command.arg(image);

    let output = bounded(command)?;
    if !output.status.success() {
        return Err(anyhow!(
            "starting {image}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Stop and remove a service's container. Absent is success.
pub fn stop(name: &str) -> Result<()> {
    let mut command = Command::new("docker");
    command.args(["rm", "-f", "-v", name]);
    let output = bounded(command)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() || stderr.contains("No such container") || stderr.contains("no such")
    {
        return Ok(());
    }
    Err(anyhow!("removing {name}: {}", stderr.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The collision the workspace dimension exists to prevent, in a third
    /// keyed resource after ports and hostnames.
    #[test]
    fn two_workspaces_do_not_share_a_container_name() {
        let a = container_name("trip/main", "db");
        let b = container_name("trip/fix-checkout", "db");
        assert_ne!(a, b);
        assert!(a.starts_with("preceipts-"), "{a}");
    }

    /// Docker names accept a narrow character set; a workspace id is a git
    /// branch and accepts almost anything.
    #[test]
    fn a_container_name_survives_an_awkward_workspace_id() {
        let name = container_name("proj/feat/JIRA-123_thing", "api");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "{name}"
        );
    }

    /// Whatever is installed, asking must return rather than hang — that is
    /// the entire point of the preflight.
    #[test]
    fn availability_answers_promptly_whatever_is_installed() {
        let started = std::time::Instant::now();
        let answer = availability();
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "availability took {:?}",
            started.elapsed()
        );
        // Any of the three is a legitimate answer on some machine; what must
        // never happen is a hang or a panic.
        assert!(!answer.explain().is_empty());
    }

    #[test]
    fn an_unavailable_runtime_explains_itself_in_terms_of_what_to_do() {
        let missing = Availability::Missing("no container runtime found".to_string());
        assert!(!missing.is_ready());
        assert!(missing.explain().contains("no container runtime"));
    }
}
