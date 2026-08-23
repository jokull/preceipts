//! The verbs that drive an environment, as library functions.
//!
//! They live in the library rather than in `main.rs` so the `preceipts` CLI
//! can call exactly what the daemon's own binary calls, with no second
//! implementation to drift.

use anyhow::{anyhow, Context, Result};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::time::Duration;

#[allow(unused_imports)]
use crate::buffer;
#[allow(unused_imports)]
use crate::ca;
use crate::cli::{EnvOp, ProcOp, TrustOp};
#[allow(unused_imports)]
use crate::client;
#[allow(unused_imports)]
use crate::config;
#[allow(unused_imports)]
use crate::daemon;
#[allow(unused_imports)]
use crate::forwarder;
#[allow(unused_imports)]
use crate::graph;
#[allow(unused_imports)]
use crate::healthcheck;
#[allow(unused_imports)]
use crate::lock;
#[allow(unused_imports)]
use crate::process;
#[allow(unused_imports)]
use crate::project;
#[allow(unused_imports)]
use crate::proto;
#[allow(unused_imports)]
use crate::proxy;
#[allow(unused_imports)]
use crate::secrets;
#[allow(unused_imports)]
use crate::services;
#[allow(unused_imports)]
use crate::share;

use crate::client as cli_client;
use crate::project::Project;
use crate::proto::{Request, Response};

pub fn resolve_root(start: PathBuf) -> Result<PathBuf> {
    let mut cur = start.canonicalize().with_context(|| "canonicalize cwd")?;
    loop {
        if crate::project::ROOT_MARKERS
            .iter()
            .any(|marker| cur.join(marker).is_file())
        {
            return Ok(cur);
        }
        let parent = cur.parent().map(|p| p.to_path_buf());
        match parent {
            Some(p) if p != cur => cur = p,
            _ => {
                return Err(anyhow!(
                    "no preceipts.toml or turbo.json found from given cwd"
                ))
            }
        }
    }
}

pub fn run_cmd(
    start: PathBuf,
    tasks: Vec<String>,
    foreground: bool,
    no_prebuild: bool,
) -> Result<()> {
    let root = resolve_root(start.clone())?;
    let state_dir = daemon::state_dir(&root);
    std::fs::create_dir_all(&state_dir)?;
    let lock_path = state_dir.join("lock");
    let socket_path = daemon::socket_path(&root);

    if let Some(pid) = lock::PidLock::read_pid(&lock_path) {
        if lock::is_alive(pid) {
            return Err(anyhow!(
                "preceipts is already running here (pid {pid}). Use `preceipts stop` first."
            ));
        }
        let _ = std::fs::remove_file(&lock_path);
    }

    // Bare `preceipts up` (no args) → service-manifest mode: bring up every
    // service declared in `preceipts.toml`. This is the canonical incantation
    // for a fully-wired repo and avoids the fan-out of bare-name expansion.
    let tasks = if tasks.is_empty() {
        let project = Project::discover(&root)?;
        let manifest: Vec<String> = project.services.tasks.keys().cloned().collect();
        if manifest.is_empty() {
            return Err(anyhow!(
                "no services given and preceipts.toml declares none. \
                 Either pass task names (`preceipts up dev`) or declare your \
                 services in preceipts.toml."
            ));
        }
        manifest
    } else {
        tasks
    };

    if foreground {
        return daemon_inner(root, tasks, no_prebuild);
    }

    // Fork-style detach via re-exec.
    let daemon = crate::daemon_exe()?;
    let mut cmd = std::process::Command::new(&daemon);
    cmd.arg("serve").arg("--root").arg(&root);
    if no_prebuild {
        cmd.arg("--no-prebuild");
    }
    cmd.args(&tasks);
    // Detach: new session, redirect stdio to /dev/null.
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            // New session — detaches from controlling terminal.
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().context("spawn daemon")?;
    let _ = child;

    // Wait for socket, then poll until every task reaches a stable state
    // (healthy / completed / terminal) or we hit a budget. Show the README
    // table on the way through.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        cli_client::wait_for_socket(&socket_path, Duration::from_secs(10)).await
    })?;
    rt.block_on(async {
        wait_and_print_status(&socket_path, Duration::from_secs(30)).await;
    });
    Ok(())
}

async fn wait_and_print_status(socket: &std::path::Path, budget: Duration) {
    let deadline = std::time::Instant::now() + budget;
    let mut last_render: Option<String> = None;
    loop {
        let resp = match cli_client::call(socket, Request::Status).await {
            Ok(r) => r,
            Err(_) => break,
        };
        let procs = match resp {
            Response::Status { procs } => procs,
            _ => break,
        };
        // Up-table is for humans driving `preceipts up`. It should answer "are
        // my services up?", not "what is every workspace package's tsc-watch
        // doing?" In a real Turborepo, requesting `dev` pulls in 20+ silent
        // `tsc --watch` builders that the user never wants to read. Surface
        // only the things they actually care about — services with a
        // hostname, services they explicitly declared in `preceipts.toml`,
        // anything that crashed (so failures aren't hidden), and the
        // turbo prebuild proc (slow + relevant). The rest get a single
        // count line so they don't disappear entirely.
        let is_service = |p: &proto::ProcStatus| -> bool {
            p.name == daemon::PREBUILD_ID
                || p.hostname.is_some()
                || p.in_manifest
                || matches!(p.state.as_str(), "crashed" | "killed")
        };
        let visible: Vec<proto::ProcStatus> =
            procs.iter().filter(|p| is_service(p)).cloned().collect();
        let hidden_count = procs
            .iter()
            .filter(|p| p.persistent && !is_service(p))
            .count();
        let stable = visible.iter().all(|p| {
            matches!(
                p.state.as_str(),
                "healthy" | "completed" | "crashed" | "killed"
            )
        });
        let rendered = render_up_table(&visible, hidden_count);
        if last_render.as_deref() != Some(rendered.as_str()) {
            // Erase prior frame.
            if let Some(prior) = &last_render {
                let lines = prior.matches('\n').count();
                for _ in 0..lines {
                    eprint!("\x1b[1A\x1b[2K");
                }
            }
            eprint!("{rendered}");
            last_render = Some(rendered);
        }
        if stable {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn render_up_table(procs: &[proto::ProcStatus], hidden_count: usize) -> String {
    let mut out = String::new();
    let mut healthy = 0usize;
    for p in procs {
        let mark = match p.state.as_str() {
            "healthy" | "completed" => {
                healthy += 1;
                "✓"
            }
            "crashed" | "killed" => "✗",
            _ => "⏳",
        };
        // The daemon's own answer, not a second one assembled here. This used
        // to compose `https://{hostname}{proxy_port}` from parts, which meant
        // the boot table happily advertised an HTTPS URL on a project with no
        // CA — where no proxy is listening and that address is a refused
        // connection.
        let host_disp = match &p.url {
            Some(url) => format!("  {url}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "{mark} {name:<28} {state:<10}{host}\n",
            name = p.name,
            state = p.state,
            host = host_disp
        ));
        for note in &p.notes {
            out.push_str(&format!("    ↳ {note}\n"));
        }
    }
    let alive = procs
        .iter()
        .filter(|p| !matches!(p.state.as_str(), "killed" | "crashed"))
        .count();
    if hidden_count > 0 {
        out.push_str(&format!(
            "  …{hidden_count} background task{plural} (workspace builders) — use `preceipts services --all` to see\n",
            plural = if hidden_count == 1 { "" } else { "s" }
        ));
    }
    out.push_str(&format!("{healthy}/{alive} tasks healthy.\n"));
    out
}

pub fn daemon_inner(root: PathBuf, tasks: Vec<String>, no_prebuild: bool) -> Result<()> {
    // Nobody is watching a daemon. Held for the whole process, so a keychain
    // read that the ACL refuses fails loudly in the log instead of stalling
    // task launch behind a dialog on someone else's screen.
    let _no_prompts = secrets::hush_prompts();

    let state_dir = daemon::state_dir(&root);
    std::fs::create_dir_all(&state_dir)?;
    let lock_path = state_dir.join("lock");
    let _lock = lock::PidLock::acquire(&lock_path)?;

    let project = Project::discover(&root)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(daemon::Daemon::run(project, tasks, state_dir, no_prebuild))
}

pub fn status_cmd(start: PathBuf, json: bool, all: bool) -> Result<()> {
    let root = resolve_root(start)?;
    let socket = daemon::socket_path(&root);
    let rt = tokio::runtime::Runtime::new()?;
    let resp = rt.block_on(cli_client::call(&socket, Request::Status))?;
    match resp {
        Response::Status { procs } => {
            if json {
                // JSON consumers (agents, scripts) get the full picture.
                println!("{}", serde_json::to_string_pretty(&procs)?);
            } else {
                let rows: Vec<&proto::ProcStatus> = if all {
                    procs.iter().collect()
                } else {
                    // Mirror the up-table service filter: hostnamed,
                    // manifest-declared, the prebuild proc, or any task in a
                    // failure state. Everything else is a workspace builder
                    // and the user can see them with `--all`.
                    procs
                        .iter()
                        .filter(|p| {
                            p.name == daemon::PREBUILD_ID
                                || p.hostname.is_some()
                                || p.in_manifest
                                || matches!(p.state.as_str(), "crashed" | "killed")
                        })
                        .collect()
                };
                println!(
                    "{:<32} {:<10} {:>8} {:>6} {:>10}  HOSTNAME",
                    "NAME", "STATE", "PID", "AGE", "LINES"
                );
                for p in rows {
                    println!(
                        "{:<32} {:<10} {:>8} {:>5}s {:>10}  {}",
                        p.name,
                        p.state,
                        p.pid.map(|x| x.to_string()).unwrap_or_else(|| "-".into()),
                        p.age_secs,
                        p.line_count,
                        p.hostname.as_deref().unwrap_or(""),
                    );
                }
            }
        }
        Response::Error { message } => return Err(anyhow!(message)),
        _ => return Err(anyhow!("unexpected response")),
    }
    Ok(())
}

pub fn wait_for_cmd(start: PathBuf, name: String, timeout: String) -> Result<()> {
    let dur = humantime::parse_duration(&timeout).map_err(|e| anyhow!("invalid --timeout: {e}"))?;
    let root = resolve_root(start)?;
    let socket = daemon::socket_path(&root);
    if !socket.exists() {
        return Err(anyhow!("no preceipts daemon running here"));
    }
    let rt = tokio::runtime::Runtime::new()?;
    let deadline = std::time::Instant::now() + dur;
    let interval = Duration::from_millis(250);
    let exit_code: i32 = rt.block_on(async {
        loop {
            let resp =
                match cli_client::call(&socket, Request::GetTask { name: name.clone() }).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("wait-for: {e}");
                        return 1;
                    }
                };
            let task = match resp {
                Response::Task { task } => task,
                Response::Error { message } => {
                    eprintln!("wait-for: {message}");
                    return 1;
                }
                _ => {
                    eprintln!("wait-for: unexpected response");
                    return 1;
                }
            };
            match task.state.as_str() {
                "healthy" | "completed" => {
                    println!("{name} is {}", task.state);
                    return 0;
                }
                "crashed" | "killed" => {
                    eprintln!(
                        "wait-for: {name} reached terminal state {} (exit {:?})",
                        task.state, task.exit_code
                    );
                    return 1;
                }
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                eprintln!(
                    "wait-for: timeout after {timeout} (last state: {})",
                    task.state
                );
                return 2;
            }
            tokio::time::sleep(interval).await;
        }
    });
    if exit_code == 0 {
        Ok(())
    } else {
        std::process::exit(exit_code);
    }
}

pub fn stop_cmd(start: PathBuf) -> Result<()> {
    let root = resolve_root(start)?;
    let socket = daemon::socket_path(&root);
    if !socket.exists() {
        println!("no running daemon");
        return Ok(());
    }
    let rt = tokio::runtime::Runtime::new()?;
    let resp = rt.block_on(cli_client::call(&socket, Request::Stop))?;
    match resp {
        Response::Ok => {
            println!("stopping…");
            // Wait briefly for socket to disappear.
            for _ in 0..50 {
                if !socket.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        }
        Response::Error { message } => Err(anyhow!(message)),
        _ => Err(anyhow!("unexpected response")),
    }
}

pub fn proc_cmd(start: PathBuf, name: String, op: ProcOp) -> Result<()> {
    let root = resolve_root(start)?;
    let socket = daemon::socket_path(&root);
    let rt = tokio::runtime::Runtime::new()?;
    match op {
        ProcOp::Tail { n, json } => {
            let resp = rt.block_on(cli_client::call(&socket, Request::Tail { name, lines: n }))?;
            print_lines(resp, json)
        }
        ProcOp::Since { cursor, json } => {
            let resp = rt.block_on(cli_client::call(&socket, Request::Since { name, cursor }))?;
            print_lines(resp, json)
        }
        ProcOp::Grep {
            pattern,
            before,
            after,
            json,
        } => {
            let resp = rt.block_on(cli_client::call(
                &socket,
                Request::Grep {
                    name: Some(name),
                    pattern,
                    before,
                    after,
                },
            ))?;
            print_grep(resp, json)
        }
        ProcOp::Signal {
            signal,
            wait,
            tail,
            json,
        } => proc_signal_cmd(&rt, &socket, name, signal, wait, tail, json),
    }
}

fn proc_signal_cmd(
    rt: &tokio::runtime::Runtime,
    socket: &std::path::Path,
    name: String,
    signal: String,
    wait: Option<String>,
    tail: bool,
    json: bool,
) -> Result<()> {
    let resp = rt.block_on(cli_client::call(
        socket,
        Request::Signal {
            name: name.clone(),
            signal: signal.clone(),
        },
    ))?;
    let initial = match resp {
        Response::Task { task } => task,
        Response::Error { message } => return Err(anyhow!(message)),
        _ => return Err(anyhow!("unexpected response")),
    };

    let Some(wait) = wait else {
        if json {
            println!("{}", serde_json::to_string_pretty(&initial)?);
        } else {
            println!(
                "sent {signal} to {name} (pid {}, state {})",
                initial
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".into()),
                initial.state
            );
        }
        return Ok(());
    };

    let dur = humantime::parse_duration(&wait).map_err(|e| anyhow!("invalid --wait: {e}"))?;
    let deadline = std::time::Instant::now() + dur;
    let mut cursor = initial.line_count;
    let mut final_task = initial.clone();

    if !json {
        println!(
            "sent {signal} to {name} (pid {}, state {}); monitoring for {wait}",
            initial
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
            initial.state
        );
    }

    rt.block_on(async {
        loop {
            if tail {
                match cli_client::call(
                    socket,
                    Request::Since {
                        name: name.clone(),
                        cursor,
                    },
                )
                .await
                {
                    Ok(Response::Lines { lines, next_cursor }) => {
                        cursor = next_cursor;
                        if !json {
                            for line in lines {
                                println!("{}", line.text);
                            }
                        }
                    }
                    Ok(Response::Error { message }) if !json => {
                        eprintln!("tail: {message}");
                    }
                    _ => {}
                }
            }

            match cli_client::call(socket, Request::GetTask { name: name.clone() }).await {
                Ok(Response::Task { task }) => {
                    final_task = task.clone();
                    if matches!(task.state.as_str(), "completed" | "crashed" | "killed") {
                        break;
                    }
                }
                Ok(Response::Error { message }) => {
                    if !json {
                        eprintln!("status: {message}");
                    }
                    break;
                }
                _ => {}
            }

            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });

    if json {
        #[derive(serde::Serialize)]
        struct Out {
            initial: proto::ProcStatus,
            final_status: proto::ProcStatus,
            next_cursor: u64,
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&Out {
                initial,
                final_status: final_task,
                next_cursor: cursor,
            })?
        );
    } else {
        println!(
            "{name} is {} (pid {}, exit {:?})",
            final_task.state,
            final_task
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
            final_task.exit_code
        );
    }

    Ok(())
}

pub fn trust_cmd(op: TrustOp) -> Result<()> {
    match op {
        TrustOp::Install { forwarder: want } => {
            ca::ensure_ca()?;
            let cert_path = ca::ca_cert_path()?;
            println!("Installing CA into /Library/Keychains/System.keychain");
            println!("  cert: {}", cert_path.display());
            println!("  This will prompt for `sudo` (Touch ID works if pam_tid is enabled).");
            let status = std::process::Command::new("sudo")
                .arg("security")
                .arg("add-trusted-cert")
                .arg("-d") // user trust → root trust (with -k System.keychain)
                .arg("-r")
                .arg("trustRoot")
                .arg("-k")
                .arg("/Library/Keychains/System.keychain")
                .arg(&cert_path)
                .status()
                .map_err(|e| anyhow!("failed to invoke `sudo security`: {e}"))?;
            if !status.success() {
                return Err(anyhow!("`security add-trusted-cert` failed"));
            }
            println!("✓ CA installed. Local https URLs are now trusted.");
            if want {
                forwarder::install()?;
            }
            Ok(())
        }
        TrustOp::Uninstall => {
            // Always attempt to tear down the forwarder — it's an additive
            // install step and `uninstall` should leave the system clean. No-op
            // if the marker files don't exist.
            if forwarder::is_installed() || forwarder::legacy_is_installed() {
                forwarder::uninstall()?;
            } else if forwarder::has_legacy_hosts_block() {
                forwarder::remove_legacy_hosts_block();
            }
            // Both names: a CA generated by procpane keeps its own common
            // name through migration, so removing only the current one would
            // leave a trusted root behind on exactly the machines that have
            // been using this longest.
            let cert_path = ca::ca_cert_path()?;
            if cert_path.is_file() {
                let mut removed = false;
                for name in [ca::CA_COMMON_NAME, ca::LEGACY_CA_COMMON_NAME] {
                    // Look before deleting. We try two names and at most one
                    // of them is ever present, so an unconditional delete
                    // guarantees that `security` prints "Unable to delete
                    // certificate matching ..." for the other — an error for
                    // the ordinary case. Reading the System keychain needs no
                    // sudo; the delete still does, and still owns the tty so
                    // its password prompt is visible.
                    let present = std::process::Command::new("security")
                        .arg("find-certificate")
                        .arg("-c")
                        .arg(name)
                        .arg("/Library/Keychains/System.keychain")
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);
                    if !present {
                        continue;
                    }
                    let status = std::process::Command::new("sudo")
                        .arg("security")
                        .arg("delete-certificate")
                        .arg("-c")
                        .arg(name)
                        .arg("-t")
                        .arg("/Library/Keychains/System.keychain")
                        .status();
                    match status {
                        Ok(s) if s.success() => {
                            println!("✓ removed \"{name}\" from System keychain");
                            removed = true;
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("sudo invocation failed: {e}"),
                    }
                }
                if !removed {
                    println!("· no CA of ours in the System keychain");
                }
            }
            if let Ok(dir) = ca::ca_dir() {
                let _ = std::fs::remove_dir_all(&dir);
                println!("✓ removed {}", dir.display());
            }
            Ok(())
        }
        TrustOp::Status => {
            if ca::is_installed() {
                println!("✓ CA files present: {}", ca::ca_dir()?.display());
            } else {
                println!("✗ CA not generated. Run `preceipts trust install` first.");
            }
            // What this build can do, read from the binary rather than assumed.
            // How secrets are protected hinges on the Team Identifier, and an
            // unsigned build failing at the moment of use is a much worse way
            // to find out.
            println!("  {}", crate::signing::KeychainStrategy::detect().explain());
            // The forwarder still goes in through sudo either way. A signed
            // bundle is *eligible* for SMAppService, which is not the same as
            // us calling it — that binding is not written yet, and a `✓` here
            // would be describing something that does not run.
            if crate::signing::can_register_daemon() {
                println!(
                    "· this bundle is eligible for SMAppService; the forwarder still \
                     installs through sudo (registration is not implemented yet)"
                );
            } else {
                println!(
                    "  the forwarder installs through sudo — SMAppService would need a \
                     signed bundle with a Team Identifier"
                );
            }

            if forwarder::is_installed() {
                println!("✓ :443 forwarder present: {}", forwarder::PROXY_PLIST_PATH);
                println!("  Portless URLs such as https://web.proj.localhost work.");
            } else {
                println!(
                    "✗ :443 forwarder not installed. Hostname URLs use :{} unless you run `preceipts trust install --forwarder`.",
                    proxy::PROXY_PORT
                );
            }
            if forwarder::legacy_is_installed() {
                println!(
                    "! a :443 helper from procpane is still registered — it may \
                     still be running, and either way `preceipts trust uninstall` \
                     removes it"
                );
            }
            // Hostnames themselves need nothing installed: macOS resolves
            // *.localhost to loopback on its own (decision 13).
            if forwarder::has_legacy_hosts_block() {
                println!(
                    "! an earlier version left a hostname block in /etc/hosts; \
                     `preceipts trust uninstall` removes it"
                );
            }
            Ok(())
        }
    }
}

pub fn env_cmd(start: PathBuf, op: EnvOp, keychain: Option<&str>) -> Result<()> {
    let root = resolve_root(start)?;
    let service = secrets::service_name(&root);
    match op {
        EnvOp::Set { key, value } => {
            validate_key(&key)?;
            let v = match value {
                Some(v) => v,
                None if !std::io::stdin().is_terminal() => {
                    // Piped/redirected stdin: read the value as a single line,
                    // stripping a trailing newline if present. Keeps
                    // `echo "$VAL" | preceipts secrets set KEY` ergonomic instead of
                    // erroring with "Device not configured" (no TTY for the
                    // password prompt).
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .map_err(|e| anyhow!("reading stdin: {e}"))?;
                    if buf.ends_with('\n') {
                        buf.pop();
                        if buf.ends_with('\r') {
                            buf.pop();
                        }
                    }
                    buf
                }
                None => rpassword::prompt_password(format!("Value for {key}: "))
                    .map_err(|e| anyhow!("prompt failed: {e} (tip: pass --value or pipe stdin)"))?,
            };
            if v.is_empty() {
                return Err(anyhow!("empty value; not storing"));
            }
            secrets::set(&service, &key, &v, keychain)?;
            println!("✓ stored {key}");
            Ok(())
        }
        EnvOp::Get { key } => {
            validate_key(&key)?;
            match secrets::get(&service, &key, keychain)? {
                Some(v) => {
                    print!("{v}");
                    Ok(())
                }
                None => Err(anyhow!("{key} not set")),
            }
        }
        EnvOp::List { json } => {
            let (keys, used_by) = discover_env_keys(&root, &service, keychain)?;

            if json {
                let rows: Vec<serde_json::Value> = keys
                    .iter()
                    .map(|k| {
                        serde_json::json!({
                            "key": k,
                            "used_by": used_by.get(k).cloned().unwrap_or_default(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if keys.is_empty() {
                println!("(no secrets stored for this repo)");
            } else {
                let width = keys.iter().map(|k| k.len()).max().unwrap_or(0);
                for k in &keys {
                    match used_by.get(k) {
                        Some(tasks) if !tasks.is_empty() => {
                            println!("{:width$}  used by: {}", k, tasks.join(", "), width = width);
                        }
                        _ => println!("{:width$}  (unused)", k, width = width),
                    }
                }
            }
            Ok(())
        }
        EnvOp::Unset { key } => {
            validate_key(&key)?;
            if secrets::delete(&service, &key, keychain)? {
                println!("✓ removed {key}");
            } else {
                println!("(no such key: {key})");
            }
            Ok(())
        }
        EnvOp::Receive => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(share::receive(&service, keychain))
        }
        EnvOp::Send { code, keys } => {
            let keys = if keys.is_empty() {
                discover_env_keys(&root, &service, keychain)?.0
            } else {
                for k in &keys {
                    validate_key(k)?;
                }
                keys
            };
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(share::send(&service, code, keys, keychain))
        }
    }
}

/// Every stored key, and which tasks reference each one.
type EnvKeyUsage = (Vec<String>, std::collections::BTreeMap<String, Vec<String>>);

fn discover_env_keys(
    root: &std::path::Path,
    service: &str,
    keychain: Option<&str>,
) -> Result<EnvKeyUsage> {
    let mut keys: std::collections::BTreeSet<String> = secrets::list_accounts(service, keychain)?
        .into_iter()
        .collect();

    // Best-effort: load the workspace so we can annotate each key with the
    // tasks that reference it in env_from. If the Keychain index is stale,
    // directly-readable env_from keys are still real stored secrets and should
    // show up in `env list` / implicit `env send`.
    let used_by: std::collections::BTreeMap<String, Vec<String>> = Project::discover(root)
        .ok()
        .map(|project| {
            let mut m: std::collections::BTreeMap<String, Vec<String>> =
                std::collections::BTreeMap::new();
            for (task_id, service) in &project.services.tasks {
                for key in &service.env_from {
                    m.entry(key.clone()).or_default().push(task_id.clone());
                }
            }
            m
        })
        .unwrap_or_default();

    for key in used_by.keys() {
        if !keys.contains(key) && matches!(secrets::get(service, key, keychain), Ok(Some(_))) {
            keys.insert(key.clone());
        }
    }

    Ok((keys.into_iter().collect(), used_by))
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(anyhow!("empty key"));
    }
    // Env-var-shaped: ASCII alnum + underscore, not starting with digit.
    let ok = key
        .chars()
        .enumerate()
        .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
    if !ok {
        return Err(anyhow!(
            "key must match [A-Za-z_][A-Za-z0-9_]* (got: {key})"
        ));
    }
    Ok(())
}

/// The HTTP the proxy carried, for a person or an agent.
///
/// Read straight off the daemon, because the transcript only exists while the
/// environment is up — it is a record of what happened, not a file that
/// accumulates. A workspace with nothing running has nothing to say, and
/// saying that plainly beats an empty table.
pub fn requests_cmd(
    start: PathBuf,
    host: Option<String>,
    since: Option<String>,
    json: bool,
) -> Result<()> {
    let root = resolve_root(start)?;
    let socket = daemon::socket_path(&root);
    let since_secs = match since {
        None => None,
        Some(text) => Some(
            humantime::parse_duration(&text)
                .map_err(|e| anyhow!("--since {text:?}: {e}"))?
                .as_secs() as i64,
        ),
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let response = rt.block_on(client::call(
        &socket,
        Request::Transcript { host, since_secs },
    ))?;
    let Response::Transcript { exchanges } = response else {
        return Err(anyhow!("unexpected response"));
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&exchanges)?);
        return Ok(());
    }
    if exchanges.is_empty() {
        println!("no requests recorded — is anything running, and has anything hit it?");
        return Ok(());
    }
    println!(
        "{:<7} {:<28} {:<6} {:>8} {:>9}  PATH",
        "STATUS", "HOST", "METHOD", "MS", "BYTES"
    );
    for exchange in &exchanges {
        // A request with no status is one that never came back. Showing it as
        // "—" rather than hiding it is the point: a hang is a finding.
        let status = exchange
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".to_string());
        let ms = exchange
            .duration_ms
            .map(|d| d.to_string())
            .unwrap_or_else(|| "—".to_string());
        println!(
            "{:<7} {:<28} {:<6} {:>8} {:>9}  {}",
            status, exchange.host, exchange.method, ms, exchange.response_bytes, exchange.path
        );
    }
    Ok(())
}

pub fn grep_cmd(
    start: PathBuf,
    pattern: String,
    before: usize,
    after: usize,
    json: bool,
) -> Result<()> {
    let root = resolve_root(start)?;
    let socket = daemon::socket_path(&root);
    let rt = tokio::runtime::Runtime::new()?;
    let resp = rt.block_on(cli_client::call(
        &socket,
        Request::Grep {
            name: None,
            pattern,
            before,
            after,
        },
    ))?;
    print_grep(resp, json)
}

fn print_lines(resp: Response, json: bool) -> Result<()> {
    match resp {
        Response::Lines { lines, next_cursor } => {
            if json {
                #[derive(serde::Serialize)]
                struct Out<'a> {
                    next_cursor: u64,
                    lines: &'a [proto::LineRecord],
                }
                let o = Out {
                    next_cursor,
                    lines: &lines,
                };
                println!("{}", serde_json::to_string_pretty(&o)?);
            } else {
                for l in lines {
                    println!("{}", l.text);
                }
                eprintln!("--- next_cursor={next_cursor} ---");
            }
            Ok(())
        }
        Response::Error { message } => Err(anyhow!(message)),
        _ => Err(anyhow!("unexpected response")),
    }
}

fn print_grep(resp: Response, json: bool) -> Result<()> {
    match resp {
        Response::GrepMatches { matches } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&matches)?);
            } else {
                for m in matches {
                    for c in &m.context_before {
                        println!("{}- {}", m.task, c);
                    }
                    println!("{}> {}", m.task, m.text);
                    for c in &m.context_after {
                        println!("{}- {}", m.task, c);
                    }
                }
            }
            Ok(())
        }
        Response::Error { message } => Err(anyhow!(message)),
        _ => Err(anyhow!("unexpected response")),
    }
}
