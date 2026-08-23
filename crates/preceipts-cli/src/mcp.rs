//! `preceipts mcp` — the lab's instruments over MCP.
//!
//! MCP is an open protocol, so this picks no side: any harness that speaks it
//! gets the lab without a plugin and without us naming a vendor. The rule from
//! docs/direction-2026-08.md holds here — **no UI-only capability** — so every
//! tool below is the same call the CLI and the panel make.
//!
//! The part worth noticing: a project's `[actions.*]` become tools
//! automatically, with their declared arguments. That is how trip's
//! `purchase-smoke` and `auth <actor>` reach an agent without a line of
//! trip-shaped code here.
//!
//! JSON-RPC 2.0 over stdio, hand-rolled. The surface is three methods, and a
//! dependency would be more code than the protocol.

use anyhow::Result;
use preceipts_core::{checks, manifest, workspace, DiffScope};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL_VERSION: &str = "2024-11-05";

pub fn serve(root: &Path) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request): std::result::Result<Value, _> = serde_json::from_str(&line) else {
            // A malformed line is the client's problem, and there is no id to
            // answer it with. Skipping beats dying mid-session.
            continue;
        };

        // Notifications carry no id and take no response.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        let response = match dispatch(root, method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32000, "message": format!("{error:#}")}
            }),
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

fn dispatch(root: &Path, method: &str, params: &Value) -> Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "preceipts", "version": env!("CARGO_PKG_VERSION")},
        })),
        "tools/list" => Ok(json!({"tools": tools(root)})),
        "tools/call" => call(root, params),
        _ => anyhow::bail!("unknown method {method}"),
    }
}

fn tool(name: &str, description: &str, properties: Value, required: Vec<&str>) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        },
    })
}

fn tools(root: &Path) -> Vec<Value> {
    let mut tools = vec![
        tool(
            "workspaces",
            "List every workspace (git worktree) of this project, with branch and recorded intent.",
            json!({}),
            vec![],
        ),
        tool(
            "where",
            "Which workspace a directory stands in. Call this first to learn which lab you are inside.",
            json!({"path": {"type": "string", "description": "Defaults to the server's root."}}),
            vec![],
        ),
        tool(
            "diff",
            "What changed in this workspace: files, statuses, and line counts against the base branch.",
            json!({
                "path": {"type": "string"},
                "uncommitted": {"type": "boolean", "description": "Compare against the working tree instead of the merge base."}
            }),
            vec![],
        ),
        tool(
            "status",
            "Receipt table for a tree: which checks are ok, failing, missing, or stale, and whether it is green.",
            json!({
                "path": {"type": "string"},
                "ref": {"type": "string", "description": "Defaults to the working tree — the tree `run` mints against."}
            }),
            vec![],
        ),
        tool(
            "run",
            "Run the project's checks and mint receipts for the current tree.",
            json!({
                "path": {"type": "string"},
                "checks": {"type": "array", "items": {"type": "string"}, "description": "Defaults to all."}
            }),
            vec![],
        ),
        tool(
            "receipts",
            "Every receipt recorded in this repository, newest first.",
            json!({"path": {"type": "string"}}),
            vec![],
        ),
        tool(
            "environment",
            "The workspace's declared environment: services, runtimes, fidelity, hostnames, drains, and any problems.",
            json!({"path": {"type": "string"}}),
            vec![],
        ),
    ];

    // The project's own verbs. This is the whole reason the schema declares
    // argument names: a tool definition can be generated from them.
    if let Ok(Some(manifest)) = manifest::load(root) {
        for action in &manifest.actions {
            let properties: serde_json::Map<String, Value> = action
                .args
                .iter()
                .map(|arg| (arg.clone(), json!({"type": "string"})))
                .collect();
            tools.push(tool(
                &action.name,
                action
                    .about
                    .as_deref()
                    .unwrap_or("A command this project declares."),
                Value::Object(properties),
                action.args.iter().map(String::as_str).collect(),
            ));
        }
    }

    tools
}

fn call(root: &Path, params: &Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tools/call needs a name"))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf());

    let result = match name {
        "workspaces" => {
            let all = workspace::discover(&path)?;
            json!(all
                .iter()
                .map(|w| json!({
                    "id": w.id,
                    "path": w.path,
                    "branch": w.branch,
                    "genesis": w.genesis,
                    "primary": w.is_primary,
                }))
                .collect::<Vec<_>>())
        }
        "where" => {
            let workspace = workspace::locate(&path)?;
            json!({
                "id": workspace.id,
                "path": workspace.path,
                "branch": workspace.branch,
                "genesis": workspace.genesis,
                "project": workspace.project_root,
            })
        }
        "diff" => {
            let scope = if args.get("uncommitted").and_then(Value::as_bool) == Some(true) {
                DiffScope::Uncommitted
            } else {
                DiffScope::Branch
            };
            let changeset = preceipts_core::load(&path, scope, false)?;
            json!({
                "base": changeset.base_name,
                "branch": changeset.branch,
                "added": changeset.total_added(),
                "removed": changeset.total_removed(),
                "files": changeset.files.iter().map(|f| json!({
                    "path": f.path,
                    "status": f.status.code().to_string(),
                    "added": f.added,
                    "removed": f.removed,
                })).collect::<Vec<_>>(),
            })
        }
        "status" => {
            let reference = args.get("ref").and_then(Value::as_str);
            let status = checks::status(&path, reference)?;
            json!({
                "ref": status.reference,
                "tree": status.tree,
                "green": status.green,
                "checks": status.rows.iter().map(|row| json!({
                    "check": row.check,
                    "required": row.required,
                    "state": row.state.as_str(),
                })).collect::<Vec<_>>(),
            })
        }
        "run" => {
            let only: Option<Vec<String>> = args.get("checks").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            });
            let report = checks::run(&path, only.as_deref())?;
            json!({
                "tree": report.tree,
                "dirty": report.dirty,
                "checks": report.outcomes.iter().map(|o| json!({
                    "check": o.name,
                    "ok": o.ok,
                    "exit": o.exit,
                    "duration_ms": o.duration.as_millis() as u64,
                    // The output is the point of asking: an agent that has to
                    // shell out for the log has been given a worse instrument.
                    "output": o.output,
                })).collect::<Vec<_>>(),
            })
        }
        "receipts" => {
            let repo = preceipts_core::notes::open(&path)?;
            let mut receipts = preceipts_core::notes::read_all_receipts(&repo);
            receipts.sort_by(|a, b| b.started.cmp(&a.started));
            json!(receipts)
        }
        "environment" => environment(&path)?,
        other => run_action(root, &path, other, &args)?,
    };

    // MCP wants human-readable content; JSON in a text block is what every
    // client renders and every model can parse.
    Ok(json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(&result)?}],
        "isError": false,
    }))
}

/// What is actually running, if a daemon is up for this project.
///
/// The declared half of `environment` is a file; this half is the reason the
/// daemon was absorbed rather than kept as a sibling tool. An agent asking
/// "is the API up" should not have to read a manifest and hope — trip's
/// discipline was to probe reality instead of trusting a stale ports file,
/// and that only works when one thing owns both halves.
///
/// A daemon that is not running is not an error: most of the time nothing is
/// up, and saying so plainly is the honest answer.
fn running_services(root: &Path) -> Value {
    use preceiptsd::proto::{Request, Response};

    let socket = preceiptsd::daemon::socket_path(root);
    if !socket.exists() {
        return json!({"up": false});
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return json!({"up": false});
    };
    match runtime.block_on(preceiptsd::client::call(&socket, Request::Status)) {
        Ok(Response::Status { procs }) => json!({
            "up": true,
            "services": procs.iter().map(|p| json!({
                "name": p.name,
                "state": p.state,
                "pid": p.pid,
                "hostname": p.hostname,
                "port": p.port,
                "url": p.url,
            })).collect::<Vec<_>>(),
        }),
        _ => json!({"up": false}),
    }
}

fn environment(path: &Path) -> Result<Value> {
    let manifest = match manifest::load(path)? {
        Some(manifest) => Some(manifest),
        None => manifest::detect(path),
    };
    let Some(manifest) = manifest else {
        return Ok(json!({
            "declared": false,
            "note": "no preceipts.toml, and nothing obvious to infer",
        }));
    };
    // The composed hostname, not the declared label: an agent that wants to
    // curl a service needs the name the fabric actually answers to.
    let workspace = workspace::locate(path).ok();
    Ok(json!({
        "declared": true,
        "running": running_services(path),
        "services": manifest.services.iter().map(|s| json!({
            "name": s.name,
            "runtime": s.runtime.as_str(),
            // Stated so an agent cannot over-claim: a browse against a mocked
            // provider is route proof, not payment proof.
            "fidelity": s.fidelity.as_str(),
            "host": s.host,
            "url": s.host.as_ref().and_then(|host| workspace
                .as_ref()
                .map(|ws| format!("https://{}", ws.hostname(host)))),
            "needs": s.needs,
        })).collect::<Vec<_>>(),
        "ready": manifest.ready,
        "drains": manifest.drains.iter().map(|d| json!({
            "name": d.name, "kind": d.kind,
        })).collect::<Vec<_>>(),
        "problems": manifest.problems(),
    }))
}

/// A verb the project declared. Arguments are substituted as `{{name}}`.
fn run_action(root: &Path, path: &Path, name: &str, args: &Value) -> Result<Value> {
    let manifest = manifest::load(root)?.ok_or_else(|| {
        anyhow::anyhow!("unknown tool {name}, and this project declares no actions")
    })?;
    let action = manifest
        .action(name)
        .ok_or_else(|| anyhow::anyhow!("unknown tool {name}"))?;

    let mut command = action.run.clone();
    for arg in &action.args {
        let value = args
            .get(arg)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("{name} needs argument \"{arg}\""))?;
        command = command.replace(&format!("{{{{{arg}}}}}"), value);
    }

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(&command)
        .current_dir(path)
        .output()?;

    Ok(json!({
        "command": command,
        "exit": output.status.code().unwrap_or(-1),
        "ok": output.status.success(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_instruments_are_all_offered() {
        let temp = tempfile::tempdir().unwrap();
        let names: Vec<String> = tools(temp.path())
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        for expected in [
            "workspaces",
            "where",
            "diff",
            "status",
            "run",
            "receipts",
            "environment",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    /// The elegant half: a project's own verbs become tools, with their
    /// declared arguments, without a line of project-specific code here.
    #[test]
    fn declared_actions_become_tools_with_their_arguments() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("preceipts.toml"),
            r#"
[actions.auth]
run = "seed-auth {{actor}}"
args = ["actor"]
about = "Switch to a seeded actor"
"#,
        )
        .unwrap();

        let tools = tools(temp.path());
        let auth = tools
            .iter()
            .find(|t| t["name"] == "auth")
            .expect("auth tool");
        assert_eq!(auth["description"], "Switch to a seeded actor");
        assert_eq!(auth["inputSchema"]["required"][0], "actor");
        assert_eq!(auth["inputSchema"]["properties"]["actor"]["type"], "string");
    }

    #[test]
    fn initialize_announces_tools() {
        let temp = tempfile::tempdir().unwrap();
        let result = dispatch(temp.path(), "initialize", &json!({})).unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "preceipts");
    }

    #[test]
    fn an_unknown_method_is_an_error_not_a_panic() {
        let temp = tempfile::tempdir().unwrap();
        assert!(dispatch(temp.path(), "nonsense", &json!({})).is_err());
    }

    #[test]
    fn an_action_substitutes_its_arguments() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("preceipts.toml"),
            "[actions.echo]\nrun = \"echo hello-{{who}}\"\nargs = [\"who\"]\n",
        )
        .unwrap();

        let result =
            run_action(temp.path(), temp.path(), "echo", &json!({"who": "world"})).unwrap();
        assert!(result["ok"].as_bool().unwrap());
        assert!(result["stdout"].as_str().unwrap().contains("hello-world"));
    }

    #[test]
    fn a_missing_action_argument_is_named() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("preceipts.toml"),
            "[actions.echo]\nrun = \"echo {{who}}\"\nargs = [\"who\"]\n",
        )
        .unwrap();
        let error = run_action(temp.path(), temp.path(), "echo", &json!({}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("who"), "{error}");
    }

    #[test]
    fn environment_states_fidelity_so_nothing_over_claims() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("preceipts.toml"),
            "[services.stripe]\nrun = \"x\"\nfidelity = \"local-simulated\"\n",
        )
        .unwrap();
        let env = environment(temp.path()).unwrap();
        assert_eq!(env["services"][0]["fidelity"], "local-simulated");
    }
}
