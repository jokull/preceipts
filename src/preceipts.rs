//! preceipts glue: this binary is the cockpit of the preceipts project (a
//! lumen fork). Engine subcommands (run/status/land/…) are delegated to the
//! TypeScript engine binary, `preceipts-engine`, so `preceipts run` and
//! `preceipts land` work from the same entry point as the cockpit.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::Command;

const ENGINE_SUBCOMMANDS: &[&str] = &["init", "run", "status", "log", "land", "hud", "sync", "gc"];

fn engine_binary() -> String {
    std::env::var("PRECEIPTS_ENGINE").unwrap_or_else(|_| "preceipts-engine".to_string())
}

/// Delegate to `preceipts-engine` when the first arg is an engine subcommand.
/// Never returns in that case: on unix the engine replaces this process.
pub fn maybe_exec_engine() {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else { return };
    let Some(name) = first.to_str() else { return };
    if !ENGINE_SUBCOMMANDS.contains(&name) {
        return;
    }

    let engine = engine_binary();
    let mut command = Command::new(&engine);
    command.arg(name).args(args);

    #[cfg(unix)]
    let err = {
        use std::os::unix::process::CommandExt;
        command.exec()
    };
    #[cfg(not(unix))]
    let err = match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(err) => err,
    };

    eprintln!(
        "error: could not run {engine}: {err}\n\
         engine subcommand \"{name}\" needs preceipts-engine on PATH (or set PRECEIPTS_ENGINE)"
    );
    std::process::exit(1);
}

/// argv for clap: a bare `preceipts` on a TTY opens the cockpit (diff view).
pub fn effective_args() -> Vec<OsString> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if args.len() == 1 && std::io::stdout().is_terminal() {
        args.push("diff".into());
    }
    args
}
