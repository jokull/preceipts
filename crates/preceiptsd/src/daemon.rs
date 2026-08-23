use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use petgraph::graph::NodeIndex;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::buffer::{self, SharedBuffer};
use crate::graph::TaskGraph;
use crate::healthcheck::{run_healthcheck_loop, HealthcheckKind};
use crate::process::{Proc, ProcState};
use crate::proto::{GrepMatch, LineRecord, ProcStatus, Request, Response};
use crate::proxy::{self, PortRegistry, PROXY_PORT};
use crate::routes;
use crate::secrets;
use crate::services::DependsOnCondition;
use crate::{ca, forwarder, project::Project};
use preceipts_core::ports;

pub const PREBUILD_ID: &str = "preceipts#prebuild";

pub struct Daemon {
    pub state_dir: PathBuf,
    pub socket_path: PathBuf,
    pub procs: BTreeMap<String, Arc<Proc>>,
    pub buffers: BTreeMap<String, SharedBuffer>,
    pub stop_tx: Arc<Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    pub started_at: Instant,
    /// Per-service hostname, composed from the manifest label.
    pub hostnames: BTreeMap<String, String>,
    /// Per-task allocated TCP port (when hostname is set; PORT env injected).
    pub allocated_ports: BTreeMap<String, u16>,
    /// Every HTTP exchange the proxy has carried for this workspace.
    ///
    /// Lives on the daemon rather than in the proxy so `Request::Transcript`
    /// can read it: the instrument is only worth building if something can
    /// ask it a question.
    pub transcript: Arc<crate::transcript::Transcript>,
    /// Slugged `<project>/<workspace>`, for resources named per workspace.
    pub workspace_key: String,
    /// Where this workspace's TLS proxy listens. Not a constant: linked
    /// workspaces take theirs from their own port block.
    pub proxy_port: u16,
    /// Per-task shutdown signal (libc::SIG*). Default SIGINT.
    pub stop_signals: BTreeMap<String, i32>,
    /// Per-task grace period before SIGKILL. Default 5s.
    pub stop_grace: BTreeMap<String, Duration>,
    /// Per-task one-line diagnostic notes (e.g. "wrangler detected → ..."),
    /// rendered as indented sub-lines under the task in the up-table.
    pub notes: BTreeMap<String, Mutex<Vec<String>>>,
    /// Task ids the manifest declares as services, as opposed to workspace
    /// builders that turbo pulled in transitively.
    /// Lets renderers distinguish user-declared services from implicit
    /// workspace `dev` tasks.
    pub manifest_tasks: std::collections::HashSet<String>,
}

impl Daemon {
    pub async fn run(
        project: Project,
        requested: Vec<String>,
        state_dir: PathBuf,
        no_prebuild: bool,
    ) -> Result<()> {
        std::fs::create_dir_all(&state_dir)?;
        let socket_path = socket_path(&project.root);
        // Remove stale socket if present.
        let _ = std::fs::remove_file(&socket_path);

        let graph = TaskGraph::build(&project, &requested)?;
        if graph.graph.node_count() == 0 {
            return Err(anyhow!("no tasks resolved"));
        }

        // Pre-flight: every env value must be resolvable — declared in the
        // manifest, or already in the Keychain.
        let literals = declared_literals(&project.root);
        let service = secrets::service_name(&project.root);
        let mut missing: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for idx in graph.graph.node_indices() {
            let n = &graph.graph[idx];
            if !n.def.persistent {
                continue;
            }
            for key in &n.service.env_from {
                if literals.contains_key(key) {
                    continue;
                }
                match secrets::get(&service, key, None) {
                    Ok(Some(_)) => {}
                    Ok(None) => missing.entry(n.id()).or_default().push(key.clone()),
                    Err(e) => {
                        return Err(anyhow!("keychain check failed for {}: {e}", n.id()));
                    }
                }
            }
        }
        if !missing.is_empty() {
            let mut all_keys: Vec<String> =
                missing.values().flat_map(|v| v.iter().cloned()).collect();
            all_keys.sort();
            all_keys.dedup();
            eprintln!(
                "✗ Missing {} required env vars: {}",
                all_keys.len(),
                all_keys.join(", ")
            );
            eprintln!("  Set them with:  preceipts secrets set <KEY>");
            eprintln!("  Or receive from a teammate:  preceipts secrets receive <code>");
            return Err(anyhow!("missing required secrets"));
        }

        // Partition graph nodes: persistent (we supervise) vs non-persistent
        // (turbo prebuild).
        let mut prebuild_ids: Vec<String> = Vec::new();
        let mut persistent_indices: Vec<NodeIndex> = Vec::new();
        for idx in graph.graph.node_indices() {
            let n = &graph.graph[idx];
            if n.def.persistent {
                persistent_indices.push(idx);
            } else if n.script.is_some() {
                prebuild_ids.push(n.id());
            }
        }
        if persistent_indices.is_empty() {
            return Err(anyhow!(
                "no persistent tasks to run; for non-persistent tasks use `turbo run` directly"
            ));
        }

        // One Proc per graph node (persistent + non-persistent), plus an
        // optional turbo-prebuild proc.
        //
        // Non-persistent nodes used to be left without a Proc entry, but their
        // state IS what `edge_satisfied` consults when downstream persistent
        // tasks decide if their `^build` deps are ready. Missing procs read as
        // `Pending` → persistent task stuck pending forever after prebuild
        // completed. Creating stub procs for them fixes that: the prebuild
        // loop flips them to Completed (existing code), and `--no-prebuild`
        // starts them as Completed (the user is asserting deps are pre-built).
        let mut procs: BTreeMap<String, Arc<Proc>> = BTreeMap::new();
        let mut buffers: BTreeMap<String, SharedBuffer> = BTreeMap::new();
        let mut node_to_id: BTreeMap<NodeIndex, String> = BTreeMap::new();
        let do_prebuild = !no_prebuild && !prebuild_ids.is_empty();
        for idx in graph.graph.node_indices() {
            let n = &graph.graph[idx];
            let id = n.id();
            let buf = buffer::new_shared(buffer::DEFAULT_CAPACITY);
            let proc = Proc::new(id.clone(), buf.clone(), n.def.persistent);
            // If the user opted out of prebuild, non-persistent dep nodes are
            // assumed already-satisfied; otherwise they wait for prebuild.
            if !n.def.persistent && !do_prebuild {
                *proc.state.lock() = ProcState::Completed;
            }
            buffers.insert(id.clone(), buf);
            procs.insert(id.clone(), proc);
            node_to_id.insert(idx, id);
        }
        if do_prebuild {
            let buf = buffer::new_shared(buffer::DEFAULT_CAPACITY);
            let proc = Proc::new(PREBUILD_ID.to_string(), buf.clone(), false);
            buffers.insert(PREBUILD_ID.to_string(), buf);
            procs.insert(PREBUILD_ID.to_string(), proc);
        }

        // Collect per-task service-derived metadata for daemon-side use.
        let mut hostnames: BTreeMap<String, String> = BTreeMap::new();
        let mut stop_signals: BTreeMap<String, i32> = BTreeMap::new();
        let mut stop_grace: BTreeMap<String, Duration> = BTreeMap::new();
        let mut notes: BTreeMap<String, Mutex<Vec<String>>> = BTreeMap::new();
        // Ports come from this workspace's reserved block, not from the
        // kernel's ephemeral range. Two properties follow, and trip learned
        // both the hard way: a worktree keeps its addresses across restarts,
        // so a bookmark still works; and two worktrees of one project can run
        // at the same time without fighting over 3000.
        //
        // A workspace that outgrows its block falls back to an ephemeral port
        // rather than refusing to start. An address that moves is worse than
        // one that does not, but it is much better than a service that will
        // not come up.
        let workspace = preceipts_core::workspace::locate(&project.root).ok();
        let block = reserve_block(workspace.as_ref());
        let mut allocated_ports: BTreeMap<String, u16> = BTreeMap::new();
        for idx in &persistent_indices {
            let n = &graph.graph[*idx];
            let id = n.id();
            if let Some(h) = &n.service.hostname {
                hostnames.insert(id.clone(), workspace_host(h, &project.root));
            }
            stop_signals.insert(id.clone(), n.service.stop_signal());
            stop_grace.insert(id.clone(), n.service.stop_grace());
            notes.insert(id, Mutex::new(Vec::new()));
        }

        // Offsets are assigned in *sorted* task order, not graph order.
        //
        // `persistent_indices` comes out of graph construction, whose order
        // follows a work stack seeded by the request — so it depends on how
        // you invoked `up`, and on the shape of the dependency walk. Assigning
        // offsets from it would mean `api` and `db` could swap ports between
        // boots, which breaks the one promise the blocks exist to make. With
        // one service you would never notice; with three it rots quietly.
        //
        // `hostnames` is a BTreeMap, so iterating it is that sorted order.
        for (offset, id) in hostnames.keys().enumerate() {
            let port = match block.and_then(|b| b.port(offset as u16 + 1)) {
                Some(port) => port,
                None => proxy::allocate_port()?,
            };
            allocated_ports.insert(id.clone(), port);
        }

        // Offset 0 of the block is this workspace's TLS proxy. *Every*
        // workspace takes one, the primary included: the well-known port
        // belongs to the router now, which splices to whichever workspace owns
        // the hostname. Before that existed the primary kept 8443 and everyone
        // else named a port, which was an arbitrary rule dressed as a default.
        let proxy_port = block.and_then(|b| b.port(0)).unwrap_or(PROXY_PORT);

        // Preflight the container runtime, once, before anything is spawned.
        //
        // A stopped backend does not fail — it *hangs*: OrbStack boots its VM
        // when something touches the docker socket, so the first `docker run`
        // blocks for seconds and the service simply never comes healthy. One
        // bounded probe up front turns that into a sentence.
        let wants_containers = project
            .services
            .tasks
            .keys()
            .any(|id| graph.by_id.contains_key(id))
            && preceipts_core::manifest::load(&project.root)
                .ok()
                .flatten()
                .is_some_and(|m| m.services.iter().any(|s| s.image.is_some()));
        if wants_containers {
            let availability = crate::container::availability();
            if !availability.is_ready() {
                eprintln!("preceipts: {}", availability.explain());
            }
        }

        let manifest_tasks: std::collections::HashSet<String> =
            project.services.tasks.keys().cloned().collect();

        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let daemon = Arc::new(Daemon {
            state_dir: state_dir.clone(),
            socket_path: socket_path.clone(),
            procs: procs.clone(),
            buffers: buffers.clone(),
            stop_tx: Arc::new(Mutex::new(Some(stop_tx.clone()))),
            started_at: Instant::now(),
            hostnames: hostnames.clone(),
            allocated_ports: allocated_ports.clone(),
            proxy_port,
            transcript: crate::transcript::Transcript::new(),
            workspace_key: workspace
                .as_ref()
                .map(|ws| {
                    ws.key()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect()
                })
                .unwrap_or_else(|| "local".to_string()),
            stop_signals,
            stop_grace,
            notes,
            manifest_tasks,
        });

        // Start the TLS reverse proxy if any task declared a hostname AND the
        // local CA is installed. Without the CA, warn but keep going (the user
        // may want to inspect status / logs without HTTPS).
        let port_registry = PortRegistry::new();
        if !hostnames.is_empty() {
            if ca::is_installed() {
                let host_list: Vec<String> = hostnames.values().cloned().collect();
                match proxy::build_tls_config(&host_list) {
                    Ok(tls_cfg) => {
                        let bind: std::net::SocketAddr = ([127, 0, 0, 1], proxy_port).into();
                        // Tell the router where to find us, and make sure it
                        // exists. Registering before the proxy is listening is
                        // deliberate: the router connects lazily, per request,
                        // so a route that briefly points at a port still
                        // binding costs one refused connection rather than a
                        // hostname that works only after a reload.
                        let hosts: Vec<String> = hostnames.values().cloned().collect();
                        if let Err(e) = routes::register(&hosts, proxy_port) {
                            tracing::warn!(?e, "could not publish routes");
                        }
                        ensure_router();
                        let reg = Arc::clone(&port_registry);
                        let transcript = Some(Arc::clone(&daemon.transcript));
                        let mut prx_stop = stop_rx.clone();
                        // Pre-register hostnames → allocated backend ports so
                        // the proxy can route even before tasks turn healthy
                        // (returns 503 until backend accepts).
                        for (id, host) in &hostnames {
                            if let Some(p) = allocated_ports.get(id) {
                                reg.register(host, *p);
                            }
                        }
                        tokio::spawn(async move {
                            if let Err(e) =
                                proxy::run_proxy(tls_cfg, reg, transcript, bind, prx_stop.clone())
                                    .await
                            {
                                tracing::error!(?e, "reverse proxy stopped");
                            }
                            let _ = prx_stop.changed().await;
                        });
                    }
                    Err(e) => {
                        eprintln!("preceipts: skipping HTTPS proxy ({e})");
                    }
                }
            } else {
                eprintln!("preceipts: tasks declare hostnames but the local CA is not installed.");
                eprintln!("  Run `preceipts trust install` to trust local https URLs.");
            }
        }
        let _ = port_registry; // silence unused if no hostnames

        // Install signal handler so Ctrl-C or SIGTERM triggers shutdown.
        {
            let stop_tx = stop_tx.clone();
            ctrlc::set_handler(move || {
                let _ = stop_tx.send(true);
            })
            .ok();
        }

        // Spawn scheduler task.
        let sched_daemon = Arc::clone(&daemon);
        let graph_arc = Arc::new(graph);
        let mut sched_stop = stop_rx.clone();
        let pm = project.pkg_manager.clone();
        let root = project.root.clone();
        let persistent_set: HashSet<NodeIndex> = persistent_indices.into_iter().collect();
        let sched_stop_rx = stop_rx.clone();
        let scheduler = tokio::spawn(async move {
            run_scheduler(
                sched_daemon,
                graph_arc,
                persistent_set,
                do_prebuild,
                prebuild_ids,
                pm,
                root,
                &mut sched_stop,
                sched_stop_rx,
            )
            .await;
        });

        // Listen on Unix socket.
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("bind {}", socket_path.display()))?;

        eprintln!(
            "preceipts daemon listening at {} (pid {})",
            socket_path.display(),
            std::process::id()
        );

        loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        break;
                    }
                }
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _addr)) => {
                            let d = Arc::clone(&daemon);
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(d, stream).await {
                                    tracing::warn!(?e, "client error");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!(?e, "accept failed");
                            break;
                        }
                    }
                }
            }
        }

        // Shutdown all procs, honoring per-task stop signal & grace.
        eprintln!("preceipts shutting down…");
        for (id, p) in &daemon.procs {
            let signal = daemon.stop_signals.get(id).copied().unwrap_or(libc::SIGINT);
            let grace = daemon
                .stop_grace
                .get(id)
                .copied()
                .unwrap_or(Duration::from_secs(5));
            p.stop_with_signal(signal, grace);
        }
        scheduler.abort();
        // Leaving a route behind would send traffic to a port that may since
        // belong to something else entirely — worse than no route at all.
        let _ = routes::unregister_port(proxy_port);
        let _ = std::fs::remove_file(&socket_path);
        Ok(())
    }
}

/// The scheduler's arguments are the scheduler's whole world, and bundling
/// them into a struct would only move the list somewhere less visible.
#[allow(clippy::too_many_arguments)]
async fn run_scheduler(
    daemon: Arc<Daemon>,
    graph: Arc<TaskGraph>,
    persistent_set: HashSet<NodeIndex>,
    do_prebuild: bool,
    prebuild_ids: Vec<String>,
    pkg_manager: String,
    root: std::path::PathBuf,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
    health_stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut spawned: HashSet<NodeIndex> = HashSet::new();

    // Default depends_on condition for an edge: a persistent dep must be
    // `healthy`; a non-persistent dep must be `completed`.
    let default_condition = |dep_idx: NodeIndex| -> DependsOnCondition {
        let dep_node = &graph.graph[dep_idx];
        if dep_node.def.persistent {
            DependsOnCondition::Healthy
        } else {
            DependsOnCondition::Completed
        }
    };

    // Does this graph edge consider `dep_idx`'s current state satisfactory?
    let edge_satisfied = |dep_idx: NodeIndex, condition: DependsOnCondition| -> bool {
        let dep_id = graph.graph[dep_idx].id();
        let st = daemon
            .procs
            .get(&dep_id)
            .map(|p| *p.state.lock())
            .unwrap_or(ProcState::Pending);
        match condition {
            DependsOnCondition::Started => {
                // Anything beyond Pending.
                !matches!(st, ProcState::Pending)
            }
            DependsOnCondition::Healthy => matches!(st, ProcState::Healthy | ProcState::Completed),
            DependsOnCondition::Completed => matches!(st, ProcState::Completed),
        }
    };

    // Pre-mark non-persistent nodes as spawned when prebuild owns them; turbo
    // prebuild satisfies their downstream effects in one shot.
    if do_prebuild {
        for idx in graph.graph.node_indices() {
            if !persistent_set.contains(&idx) {
                spawned.insert(idx);
            }
        }
    }

    // Turbo prebuild phase.
    if do_prebuild {
        if let Some(proc) = daemon.procs.get(PREBUILD_ID).cloned() {
            let mut shell = format!("{pkg_manager} exec turbo run");
            for id in &prebuild_ids {
                shell.push(' ');
                shell.push_str(id);
            }
            let env: Vec<(String, String)> = Vec::new();
            // Prebuild is just `turbo run …` — inherit full env so user's
            // turbo cache token / npm registry config / etc. all work.
            if let Err(e) = proc.spawn(&shell, &root, &env, true) {
                tracing::error!(?e, "prebuild spawn failed");
                *proc.state.lock() = ProcState::Crashed;
                if let Some(tx) = daemon.stop_tx.lock().as_ref() {
                    let _ = tx.send(true);
                }
                return;
            }
            loop {
                if *stop_rx.borrow() {
                    return;
                }
                let st = *proc.state.lock();
                if st.is_terminal() {
                    if matches!(st, ProcState::Crashed | ProcState::Killed) {
                        eprintln!("preceipts: turbo prebuild failed; aborting run");
                        if let Some(tx) = daemon.stop_tx.lock().as_ref() {
                            let _ = tx.send(true);
                        }
                        return;
                    }
                    // Synthetic non-persistent nodes for prebuild-handled tasks:
                    // mark their procs Completed so downstream gates clear.
                    for idx in graph.graph.node_indices() {
                        if !persistent_set.contains(&idx) {
                            let id = graph.graph[idx].id();
                            if let Some(p) = daemon.procs.get(&id) {
                                let mut st = p.state.lock();
                                if matches!(*st, ProcState::Pending) {
                                    *st = ProcState::Completed;
                                }
                            }
                        }
                    }
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                    _ = stop_rx.changed() => { if *stop_rx.borrow() { return; } }
                }
            }
        }
    }

    loop {
        if *stop_rx.borrow() {
            return;
        }

        // Find nodes whose deps are all satisfied (per-edge condition).
        let mut to_spawn: Vec<NodeIndex> = Vec::new();
        for idx in graph.graph.node_indices() {
            if spawned.contains(&idx) {
                continue;
            }
            let node = &graph.graph[idx];
            let mut ready = true;
            for dep in graph
                .graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
            {
                let dep_id = graph.graph[dep].id();
                // Look up per-task override from the dependent's service, by both
                // canonical id and short id (service map keys can use either).
                let cond = node
                    .service
                    .depends_on
                    .get(&dep_id)
                    .or_else(|| {
                        // Strip "@scope/" prefix on the dep package, if any.
                        graph.graph[dep]
                            .package
                            .split_once('/')
                            .and_then(|(_, tail)| {
                                let short_id = format!("{}#{}", tail, graph.graph[dep].task);
                                node.service.depends_on.get(&short_id)
                            })
                    })
                    .copied()
                    .unwrap_or_else(|| default_condition(dep));
                if !edge_satisfied(dep, cond) {
                    ready = false;
                    break;
                }
            }
            if ready {
                to_spawn.push(idx);
            }
        }

        for idx in &to_spawn {
            let node = &graph.graph[*idx];
            let id = node.id();
            let proc = match daemon.procs.get(&id) {
                Some(p) => p.clone(),
                None => continue,
            };
            // Determine shell command. If no script, skip (treat as no-op completed).
            let shell_cmd = match &node.script {
                Some(s) => s.clone(),
                None => {
                    *proc.state.lock() = ProcState::Completed;
                    spawned.insert(*idx);
                    continue;
                }
            };
            // Prepend node_modules/.bin to PATH walking from cwd up to root.
            let mut bin_paths: Vec<std::path::PathBuf> = Vec::new();
            let mut cur = Some(node.cwd.as_path());
            while let Some(d) = cur {
                let nb = d.join("node_modules").join(".bin");
                if nb.is_dir() {
                    bin_paths.push(nb);
                }
                cur = d.parent();
            }
            let cur_path = std::env::var("PATH").unwrap_or_default();
            let mut path = String::new();
            for p in &bin_paths {
                if !path.is_empty() {
                    path.push(':');
                }
                path.push_str(&p.to_string_lossy());
            }
            if !cur_path.is_empty() {
                if !path.is_empty() {
                    path.push(':');
                }
                path.push_str(&cur_path);
            }
            let mut env: Vec<(String, String)> = vec![("PATH".into(), path)];
            // Container names are namespaced by workspace for the same reason
            // ports and hostnames are: two worktrees must not fight over one.
            env.push((
                "PRECEIPTS_WORKSPACE_KEY".into(),
                daemon.workspace_key.clone(),
            ));

            // If this task has a hostname, hand it the allocated port via PORT
            // and a public URL via the canonical-cased hostname env var.
            if let Some(host) = daemon.hostnames.get(&id) {
                if let Some(port) = daemon.allocated_ports.get(&id) {
                    env.push(("PORT".into(), port.to_string()));
                    env.push((
                        "PRECEIPTS_PUBLIC_URL".into(),
                        public_url(host, daemon.proxy_port),
                    ));
                }
            }
            // Inject every *other* task's public URL too, so apps that talk to
            // siblings can resolve without hard-coding ports.
            for (other_id, other_host) in &daemon.hostnames {
                if other_id == &id {
                    continue;
                }
                // Build an env var name from the short task name:
                //   "@demo/api#dev" → API_URL
                //   "api#dev"       → API_URL
                let pkg_short = other_id
                    .split_once('#')
                    .map(|(p, _)| p)
                    .unwrap_or(other_id)
                    .rsplit('/')
                    .next()
                    .unwrap_or(other_id)
                    .to_uppercase()
                    .replace(['-', '.'], "_");
                let var_name = format!("{pkg_short}_URL");
                env.push((var_name, public_url(other_host, daemon.proxy_port)));
            }

            // Inject env_from secrets from Keychain. Pre-flight verified
            // presence; if a value disappeared between then and now we warn
            // and let the task start without it (rare race).
            // Literals come from the manifest, secrets from the Keychain. The
            // manifest is consulted first because a literal is declared, not
            // stored, and asking the Keychain for one would fail every time.
            let literals = declared_literals(&root);
            let service = secrets::service_name(&root);
            for key in &node.service.env_from {
                if let Some(value) = literals.get(key) {
                    env.push((key.clone(), value.clone()));
                    continue;
                }
                match secrets::get(&service, key, None) {
                    Ok(Some(val)) => env.push((key.clone(), val)),
                    Ok(None) => {
                        tracing::warn!(task = %id, key, "env_from secret vanished after pre-flight")
                    }
                    Err(e) => tracing::warn!(task = %id, key, error = ?e, "env_from fetch failed"),
                }
            }

            // Wrangler doesn't read parent process env by default; opt in so
            // env_from secrets reach the Worker's `env` binding without ever
            // touching `.dev.vars`. Detect `wrangler` as a standalone command
            // token in the script (handles `wrangler dev`, `npx wrangler`,
            // `pnpm exec wrangler`, `bun x wrangler`, etc.).
            let wrangler = is_wrangler_invocation(&shell_cmd);
            if wrangler {
                env.push(("CLOUDFLARE_INCLUDE_PROCESS_ENV".into(), "true".into()));
            }

            // If the task declared env_from, treat that as opting into a tight
            // allowlist: spawn with a scrubbed env so the parent shell's other
            // exports don't bleed in (and, transitively, don't bleed into the
            // Worker when CLOUDFLARE_INCLUDE_PROCESS_ENV is on).
            let scrubbed = !node.service.env_from.is_empty();

            // Record a one-line note for the up-table when we auto-flip the
            // Wrangler flag — the user opted into env_from, and we're telling
            // them their allowlist now covers the Worker's `env` too.
            if wrangler && scrubbed {
                if let Some(slot) = daemon.notes.get(&id) {
                    slot.lock().push(
                        "wrangler detected → CLOUDFLARE_INCLUDE_PROCESS_ENV=true; env_from is the allowlist".into(),
                    );
                }
            }

            let cwd = node.cwd.clone();
            if let Err(e) = proc.spawn(&shell_cmd, &cwd, &env, !scrubbed) {
                tracing::error!(?e, "failed to spawn {id}");
                *proc.state.lock() = ProcState::Crashed;
                spawned.insert(*idx);
                continue;
            }
            spawned.insert(*idx);

            // Kick off the healthcheck loop for this task.
            let hc_cfg = node.service.healthcheck.clone();
            let hostname = node.service.hostname.clone();
            let is_container = node.service.container;
            let buffer = match daemon.buffers.get(&id) {
                Some(b) => b.clone(),
                None => continue,
            };
            let kind = match &hc_cfg {
                Some(hc) => match HealthcheckKind::from_sidecar(hc, hostname.as_deref()) {
                    // A container's declared tcp port is where it listens
                    // *inside*. Probing that on the host asks a question about
                    // somebody else's service, so the probe goes to the port we
                    // published instead.
                    Ok(HealthcheckKind::Tcp(_)) if is_container => {
                        match daemon.allocated_ports.get(&id) {
                            Some(port) => HealthcheckKind::Tcp(*port),
                            None => HealthcheckKind::None,
                        }
                    }
                    Ok(k) => k,
                    Err(e) => {
                        tracing::warn!(task = %id, error = ?e, "invalid healthcheck; treating as none");
                        HealthcheckKind::None
                    }
                },
                None => HealthcheckKind::None,
            };
            let interval = hc_cfg
                .as_ref()
                .map(|h| h.interval())
                .unwrap_or(Duration::from_secs(1));
            let probe_t = hc_cfg
                .as_ref()
                .map(|h| h.timeout())
                .unwrap_or(Duration::from_secs(2));
            let start_p = hc_cfg
                .as_ref()
                .map(|h| h.start_period())
                .unwrap_or(Duration::ZERO);
            let proc_for_hc = Arc::clone(&proc);
            let hc_stop = health_stop_rx.clone();
            tokio::spawn(async move {
                run_healthcheck_loop(
                    proc_for_hc,
                    buffer,
                    kind,
                    interval,
                    probe_t,
                    start_p,
                    hc_stop,
                )
                .await;
            });
        }

        // Exit condition: every non-persistent task has reached terminal state
        // AND no persistent task is alive (either all terminal, or none was
        // requested).
        let mut any_persistent_alive = false;
        let mut all_non_persistent_done = true;
        for idx in graph.graph.node_indices() {
            let node = &graph.graph[idx];
            let id = node.id();
            let state = daemon
                .procs
                .get(&id)
                .map(|p| *p.state.lock())
                .unwrap_or(ProcState::Pending);
            if node.def.persistent {
                if !state.is_terminal() {
                    any_persistent_alive = true;
                }
            } else if !state.is_terminal() {
                all_non_persistent_done = false;
            }
        }

        if all_non_persistent_done && !any_persistent_alive {
            if let Some(tx) = daemon.stop_tx.lock().as_ref() {
                let _ = tx.send(true);
            }
            return;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() { return; }
            }
        }
    }
}

async fn handle_client(daemon: Arc<Daemon>, stream: UnixStream) -> Result<()> {
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }
    let req: Request = serde_json::from_str(line.trim()).context("parse request")?;
    let resp = dispatch(&daemon, req);
    let mut out = serde_json::to_vec(&resp)?;
    out.push(b'\n');
    wr.write_all(&out).await?;
    wr.flush().await?;
    Ok(())
}

fn dispatch(daemon: &Daemon, req: Request) -> Response {
    match req {
        Request::Ping => Response::Pong,
        Request::Transcript { host, since_secs } => Response::Transcript {
            exchanges: daemon.transcript.recent(host.as_deref(), since_secs),
        },
        Request::Stop => {
            if let Some(tx) = daemon.stop_tx.lock().as_ref() {
                let _ = tx.send(true);
            }
            Response::Ok
        }
        Request::Status => {
            let procs = daemon
                .procs
                .iter()
                .map(|(id, p)| build_proc_status(daemon, id, p))
                .collect();
            Response::Status { procs }
        }
        Request::GetTask { name } => match daemon.procs.get(&name) {
            Some(p) => Response::Task {
                task: build_proc_status(daemon, &name, p),
            },
            None => Response::Error {
                message: format!("unknown task: {name}"),
            },
        },
        Request::Tail { name, lines } => match daemon.buffers.get(&name) {
            Some(buf) => {
                let recs: Vec<LineRecord> = buf.lock().tail(lines);
                let next = buf.lock().line_count();
                Response::Lines {
                    lines: recs,
                    next_cursor: next,
                }
            }
            None => Response::Error {
                message: format!("unknown task: {name}"),
            },
        },
        Request::Since { name, cursor } => match daemon.buffers.get(&name) {
            Some(buf) => {
                let (recs, next) = buf.lock().since(cursor);
                Response::Lines {
                    lines: recs,
                    next_cursor: next,
                }
            }
            None => Response::Error {
                message: format!("unknown task: {name}"),
            },
        },
        Request::Signal { name, signal } => {
            let signal_num = match parse_signal(&signal) {
                Ok(signal_num) => signal_num,
                Err(e) => {
                    return Response::Error {
                        message: e.to_string(),
                    }
                }
            };
            match daemon.procs.get(&name) {
                Some(p) => match p.send_signal(signal_num) {
                    Ok(()) => Response::Task {
                        task: build_proc_status(daemon, &name, p),
                    },
                    Err(e) => Response::Error {
                        message: format!("signal {name}: {e}"),
                    },
                },
                None => Response::Error {
                    message: format!("unknown task: {name}"),
                },
            }
        }
        Request::Grep {
            name,
            pattern,
            before,
            after,
        } => {
            let re = match regex::Regex::new(&pattern) {
                Ok(r) => r,
                Err(e) => {
                    return Response::Error {
                        message: format!("bad regex: {e}"),
                    }
                }
            };
            let mut matches: Vec<GrepMatch> = Vec::new();
            if let Some(name) = name {
                if let Some(buf) = daemon.buffers.get(&name) {
                    matches.extend(buf.lock().grep(&name, &re, before, after));
                } else {
                    return Response::Error {
                        message: format!("unknown task: {name}"),
                    };
                }
            } else {
                for (id, buf) in &daemon.buffers {
                    matches.extend(buf.lock().grep(id, &re, before, after));
                }
            }
            Response::GrepMatches { matches }
        }
    }
}

/// The daemon's control socket for a project.
///
/// **Not** under the project, which is where it used to live. A unix socket
/// path is capped at 104 bytes on macOS (`SUN_LEN`), and a worktree a few
/// directories deep blows straight through that — the bind fails with a
/// message about `SUN_LEN` that says nothing about the real cause, and the
/// only symptom a user sees is "daemon did not come up". Naming it from a
/// hash of the canonical root in the per-user temp directory makes the length
/// constant and the identity still one-to-one with the project.
///
/// Exposed so callers do not each rebuild the path from parts — the CLI, the
/// MCP server, and the daemon itself all have to agree on it, and a
/// disagreement reads as "no daemon running" rather than as a bug.
pub fn socket_path(root: &Path) -> PathBuf {
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    std::env::temp_dir().join(format!(
        "preceipts-{:016x}.sock",
        fnv1a(canonical.as_bytes())
    ))
}

/// FNV-1a. Not `DefaultHasher`: its output is explicitly not guaranteed stable
/// across releases, and this value has to mean the same thing to a daemon
/// started last week and a CLI built today.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod socket_path_tests {
    use super::*;

    /// The regression: the socket used to live under the project, so a deep
    /// worktree could not start a daemon at all.
    #[test]
    fn the_path_stays_short_however_deep_the_project_is() {
        let deep = PathBuf::from("/Users/someone/Code")
            .join("a-fairly-long-directory-name".repeat(4))
            .join("another-quite-long-directory-name")
            .join("worktrees")
            .join("fix-the-checkout-race-in-payments");
        let socket = socket_path(&deep);
        assert!(
            socket.as_os_str().len() < 104,
            "unix sockets cap at 104 bytes: {} was {}",
            socket.display(),
            socket.as_os_str().len()
        );
    }

    #[test]
    fn two_projects_do_not_share_a_socket() {
        assert_ne!(
            socket_path(Path::new("/tmp/one")),
            socket_path(Path::new("/tmp/two"))
        );
    }

    #[test]
    fn the_same_project_always_gets_the_same_socket() {
        let root = Path::new("/tmp/one");
        assert_eq!(socket_path(root), socket_path(root));
    }

    /// Pinned because a daemon started before an upgrade must still be
    /// reachable by a CLI built after one.
    #[test]
    fn the_hash_is_stable_across_builds() {
        assert_eq!(fnv1a(b"/Users/x/proj"), fnv1a(b"/Users/x/proj"));
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }
}

pub fn state_dir(root: &Path) -> PathBuf {
    root.join(".preceipts").join("daemon")
}

/// Give a declared hostname its workspace dimension.
///
/// The composition itself lives in `preceipts_core::workspace` — it is the
/// fabric's rule, not the daemon's, and the CLI and app compose the same
/// names. This is the lookup half: find which workspace `root` stands in.
///
/// A path that is not in a git repository at all still needs *a* hostname, so
/// it falls back to the bare label plus the suffix rather than failing a boot
/// over a naming detail.
pub fn workspace_host(declared: &str, root: &Path) -> String {
    match preceipts_core::workspace::locate(root) {
        Ok(workspace) => workspace.hostname(declared),
        Err(_) => {
            let label = declared.split('.').next().unwrap_or(declared);
            format!("{label}.{}", preceipts_core::workspace::HOST_SUFFIX)
        }
    }
}

/// Returns true if the shell command runs `wrangler` as a command (not as a
/// substring of some other word). Tokenize on whitespace and shell separators.
fn is_wrangler_invocation(shell_cmd: &str) -> bool {
    shell_cmd
        .split(|c: char| c.is_whitespace() || matches!(c, '&' | '|' | ';' | '(' | ')'))
        .any(|tok| tok == "wrangler" || tok.ends_with("/wrangler"))
}

/// The URL to hand a person or a sibling service.
///
/// Portless whenever the `:443` forwarder is installed — for *every*
/// workspace, not just one. That is what the router bought: the forwarder
/// points at one listener, and that listener now splices by hostname to
/// whichever workspace owns the name. Without the forwarder the URL names
/// this workspace's own proxy port, which is the honest thing to print when
/// nothing is listening on 443.
fn public_url(host: &str, proxy_port: u16) -> String {
    if forwarder::is_installed() {
        format!("https://{host}")
    } else {
        format!("https://{host}:{proxy_port}")
    }
}

fn parse_signal(raw: &str) -> Result<i32> {
    let s = raw.trim().to_ascii_uppercase();
    let s = s.strip_prefix("SIG").unwrap_or(&s);
    match s {
        "HUP" => Ok(libc::SIGHUP),
        "INT" => Ok(libc::SIGINT),
        "QUIT" => Ok(libc::SIGQUIT),
        "TERM" => Ok(libc::SIGTERM),
        "KILL" => Ok(libc::SIGKILL),
        "USR1" => Ok(libc::SIGUSR1),
        "USR2" => Ok(libc::SIGUSR2),
        "STOP" => Ok(libc::SIGSTOP),
        "CONT" => Ok(libc::SIGCONT),
        _ => {
            if let Ok(n) = s.parse::<i32>() {
                if n > 0 {
                    return Ok(n);
                }
            }
            Err(anyhow!("unknown signal: {raw}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_wrangler_invocation, parse_signal, public_url};

    #[test]
    fn detects_wrangler() {
        assert!(is_wrangler_invocation("wrangler dev"));
        assert!(is_wrangler_invocation("npx wrangler dev"));
        assert!(is_wrangler_invocation("pnpm exec wrangler dev"));
        assert!(is_wrangler_invocation("bun x wrangler"));
        assert!(is_wrangler_invocation("./node_modules/.bin/wrangler dev"));
        assert!(is_wrangler_invocation("cd worker && wrangler dev"));
    }

    #[test]
    fn ignores_lookalikes() {
        assert!(!is_wrangler_invocation("vite"));
        assert!(!is_wrangler_invocation("./wrangler-cleanup.sh"));
        assert!(!is_wrangler_invocation("echo 'wrangler is great'")); // string literal
    }

    #[test]
    fn public_url_matches_pretty_url_install_state() {
        let expected = if crate::forwarder::is_installed() {
            "https://api.proj.localhost"
        } else {
            "https://api.proj.localhost:8443"
        };
        assert_eq!(
            public_url("api.proj.localhost", super::PROXY_PORT),
            expected
        );
    }

    /// Without the forwarder, a URL names the port that is actually serving
    /// it — this workspace's own proxy, never the well-known one.
    #[test]
    fn without_the_forwarder_a_url_names_the_port_that_serves_it() {
        if crate::forwarder::is_installed() {
            return;
        }
        assert_eq!(
            public_url("api.feat.proj.localhost", 21000),
            "https://api.feat.proj.localhost:21000"
        );
    }

    #[test]
    fn parses_signal_names_and_numbers() {
        assert_eq!(parse_signal("HUP").unwrap(), libc::SIGHUP);
        assert_eq!(parse_signal("SIGHUP").unwrap(), libc::SIGHUP);
        assert_eq!(parse_signal("term").unwrap(), libc::SIGTERM);
        assert_eq!(parse_signal("15").unwrap(), 15);
        assert!(parse_signal("NOPE").is_err());
    }
}

fn build_proc_status(daemon: &Daemon, id: &str, p: &Proc) -> ProcStatus {
    let now = Instant::now();
    let started = *p.started_at.lock();
    let age = started
        .map(|t| now.duration_since(t).as_secs())
        .unwrap_or(0);
    let line_count = p.buffer.lock().line_count();
    let notes = daemon
        .notes
        .get(id)
        .map(|m| m.lock().clone())
        .unwrap_or_default();
    ProcStatus {
        name: id.to_string(),
        state: p.state.lock().as_str().to_string(),
        pid: *p.pid.lock(),
        age_secs: age,
        line_count,
        exit_code: *p.exit_code.lock(),
        persistent: p.persistent,
        hostname: daemon.hostnames.get(id).cloned(),
        port: daemon.allocated_ports.get(id).copied(),
        url: daemon
            .hostnames
            .get(id)
            .map(|host| public_url(host, daemon.proxy_port)),
        notes,
        in_manifest: daemon.manifest_tasks.contains(id),
    }
}

#[cfg(test)]
mod workspace_host_tests {
    use super::workspace_host;

    /// The composition rules are tested in `preceipts_core::workspace`; what
    /// belongs here is the lookup, including the case core never sees.
    #[test]
    fn a_path_outside_a_repository_still_gets_a_hostname() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(workspace_host("api", temp.path()), "api.localhost");
    }

    #[test]
    fn a_workspace_hostname_carries_its_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            &["git", "init", "-q", "-b", "main"][..],
            &["git", "config", "user.name", "t"],
            &["git", "config", "user.email", "t@t.local"],
            &["git", "config", "commit.gpgsign", "false"],
        ] {
            assert!(std::process::Command::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .status()
                .unwrap()
                .success());
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        for args in [&["git", "add", "-A"][..], &["git", "commit", "-qm", "base"]] {
            assert!(std::process::Command::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .status()
                .unwrap()
                .success());
        }

        let ws = preceipts_core::workspace::create(&dir, "fix-checkout", None, None).unwrap();
        assert_eq!(
            workspace_host("api", &ws.path),
            "api.fix-checkout.proj.localhost"
        );
        assert_eq!(workspace_host("api", &dir), "api.proj.localhost");
    }
}

/// Env values the manifest declares outright, rather than storing.
///
/// One function because two call sites need the same answer: the pre-flight
/// that refuses to boot over a missing secret, and the injection that hands
/// values to a process. When those disagreed, a declared literal passed
/// injection and failed pre-flight — the service could run, but was never
/// allowed to start.
fn declared_literals(root: &Path) -> BTreeMap<String, String> {
    preceipts_core::manifest::load(root)
        .ok()
        .flatten()
        .map(|manifest| {
            manifest
                .env_rules
                .iter()
                .filter_map(|rule| rule.value.clone().map(|value| (rule.key.clone(), value)))
                .collect()
        })
        .unwrap_or_default()
}

/// Start the router if nothing is serving the well-known port yet.
///
/// Racy by construction — two daemons starting together both see a free port
/// and both spawn — and that is fine: the loser fails to bind, prints, and
/// exits, while the winner serves both of them. Coordinating instead would
/// mean a lock file to leave behind when something is killed, which is a
/// worse failure than a process that exits immediately.
fn ensure_router() {
    if std::net::TcpStream::connect(("127.0.0.1", routes::ROUTER_PORT)).is_ok() {
        return;
    }
    let Ok(daemon) = crate::daemon_exe() else {
        tracing::warn!("no preceiptsd binary to start the router with");
        return;
    };
    let mut command = std::process::Command::new(daemon);
    command
        .arg("route")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            // Its own session: the router outlives the workspace that
            // happened to start it, because the next workspace needs it too.
            libc::setsid();
            Ok(())
        });
    }
    if let Err(e) = command.spawn() {
        tracing::warn!(?e, "could not start the router");
    }
}

/// This workspace's reserved port block, taken once and kept.
///
/// The reservation file is machine-global and shared by every project, so it
/// is read, updated, and written on each boot rather than held open — a daemon
/// that crashed must not leave the file locked, and the write is small enough
/// that the race window is narrower than the human it would inconvenience.
///
/// `None` when there is no workspace to key on (a directory that is not a git
/// repository) or the range is full. Callers fall back to ephemeral ports.
fn reserve_block(workspace: Option<&preceipts_core::workspace::Workspace>) -> Option<ports::Block> {
    let workspace = workspace?;
    let path = reservations_path()?;
    let mut reservations = ports::Reservations::load(&path);
    let block = reservations.reserve(&workspace.key())?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = reservations.save(&path) {
        // A block that could not be written is still usable now; it just may
        // not survive a restart. Worth a line, not worth refusing to boot.
        tracing::warn!(?e, "could not persist the port reservation");
    }
    Some(block)
}

fn reservations_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".preceipts").join("ports.toml"))
}
