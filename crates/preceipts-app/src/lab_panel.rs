//! X-ray on the workspace: what is running, on what URL, and what it is saying.
//!
//! This is the "laboratory" half of docs/direction-2026-08.md made visible.
//! Everything here is read from the daemon over the same socket the CLI and
//! the MCP server use — there is no UI-only fact on this panel, which is the
//! rule that keeps the window and the agent looking at the same world.
//!
//! It polls. The protocol has no subscribe, and adding one is a daemon change
//! that should be made when something needs it rather than because a panel
//! exists. Two calls a second and a cursor on the log tail is not a load worth
//! designing around.
//!
//! Nothing here starts, stops, or restarts anything yet. The app is a read
//! surface with a control plane, and the control plane is the part that has to
//! be earned one verb at a time.

use crate::daemon;
use gpui::prelude::*;
use gpui::{div, px, ClipboardItem, Context, ScrollHandle, SharedString, Window};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::divider::Divider;
use gpui_component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use preceipts_core::workspace::Workspace;
use preceipts_proto::{Exchange, LineRecord, ProcStatus, Request, Response};
use std::path::PathBuf;
use std::time::Duration;

/// How often the panel asks. Fast enough that a service coming up feels
/// immediate, slow enough to be free.
const POLL: Duration = Duration::from_millis(1200);

/// How much log the panel keeps. The daemon's own ring buffer is the archive;
/// this is a window onto its end.
const LOG_LINES: usize = 400;

/// Which instrument the lower half is showing.
///
/// Both are things the daemon already records for its own reasons — the ring
/// buffers because a service's output has to go somewhere, the transcript
/// because the TLS proxy is in the path of every request anyway. Neither
/// costs the code under test anything, which is what makes them usable on a
/// Rails app, a Go binary and a Vite dev server identically.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Instrument {
    Output,
    Requests,
}

pub struct LabPanel {
    workspace: Workspace,
    socket: PathBuf,
    /// The daemon's check-run state, refreshed on the same poll as everything
    /// else. The pane's Run button reads it — a second poll for one boolean
    /// would be a second thing to keep in step.
    checks: preceipts_proto::ChecksState,
    /// `Err` is the ordinary state — most projects have no environment up.
    services: Result<Vec<ProcStatus>, String>,
    /// Whose output is in the lower half.
    selected: Option<String>,
    lines: Vec<LineRecord>,
    cursor: u64,
    instrument: Instrument,
    exchanges: Vec<Exchange>,
    /// The log is a tail, so it lives at the bottom.
    log_scroll: ScrollHandle,
    /// Resolved once. A `render` has no business asking the text system
    /// anything — see the note on `theme::code_font`.
    font: SharedString,
    copied: bool,
}

impl LabPanel {
    pub fn new(workspace: Workspace, cx: &mut Context<Self>) -> Self {
        // The *daemon's* notion of the project, which is the nearest marker
        // above the worktree — not the git root. A linked worktree carries its
        // own manifest and therefore its own environment.
        let root = preceipts_proto::project_root(&workspace.path)
            .unwrap_or_else(|| workspace.project_root.clone());
        let socket = preceipts_proto::socket_path(&root);
        let this = Self {
            workspace,
            socket,
            checks: Default::default(),
            services: Err("no daemon".to_string()),
            selected: None,
            lines: Vec::new(),
            cursor: 0,
            instrument: Instrument::Output,
            exchanges: Vec::new(),
            log_scroll: ScrollHandle::new(),
            font: crate::theme::code_font(cx),
            copied: false,
        };
        this.poll(cx);
        this
    }

    /// One loop, both queries. Every call is blocking, so every call goes to
    /// the background executor and only the result comes back to the window.
    fn poll(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            let Ok((socket, selected, cursor, instrument)) = this.read_with(cx, |this, _| {
                (
                    this.socket.clone(),
                    this.selected.clone(),
                    this.cursor,
                    this.instrument,
                )
            }) else {
                return;
            };

            let status = cx
                .background_executor()
                .spawn({
                    let socket = socket.clone();
                    async move { daemon::call(&socket, &Request::Status) }
                })
                .await;

            let checks = cx
                .background_executor()
                .spawn({
                    let socket = socket.clone();
                    async move { daemon::call(&socket, &Request::Checks) }
                })
                .await;

            let instrument_result = match (instrument, selected) {
                (Instrument::Output, Some(name)) => Some(
                    cx.background_executor()
                        .spawn(
                            async move { daemon::call(&socket, &Request::Since { name, cursor }) },
                        )
                        .await,
                ),
                (Instrument::Requests, _) => Some(
                    cx.background_executor()
                        .spawn(async move {
                            daemon::call(
                                &socket,
                                &Request::Transcript {
                                    host: None,
                                    since_secs: None,
                                },
                            )
                        })
                        .await,
                ),
                _ => None,
            };

            if this
                .update(cx, |this, cx| {
                    this.services = match status {
                        Ok(Response::Status { procs }) => Ok(procs),
                        Ok(Response::Error { message }) => Err(message),
                        Ok(_) => Err("unexpected answer".to_string()),
                        Err(message) => Err(message),
                    };
                    // A daemon that is not there is not a run that stopped:
                    // absence leaves the state default, which reads as idle.
                    this.checks = match checks {
                        Ok(Response::Checks { state }) => state,
                        _ => Default::default(),
                    };
                    match instrument_result {
                        Some(Ok(Response::Lines { lines, next_cursor })) => {
                            let arrived = !lines.is_empty();
                            this.lines.extend(lines);
                            // Keep the tail bounded: a dev server left running
                            // overnight is a few hundred thousand lines.
                            if this.lines.len() > LOG_LINES {
                                this.lines.drain(..this.lines.len() - LOG_LINES);
                            }
                            this.cursor = next_cursor;
                            // A tail that does not follow is a scrollback you
                            // have to chase. Unconditional on purpose for now:
                            // "am I scrolled up on purpose" is a real question
                            // and deserves a real answer, not a guess.
                            if arrived {
                                this.log_scroll.scroll_to_bottom();
                            }
                        }
                        Some(Ok(Response::Transcript { exchanges })) => {
                            this.exchanges = exchanges;
                        }
                        _ => {}
                    }
                    // Land on something the moment there is something to land
                    // on. X-ray vision that waits to be asked is a menu.
                    if this.selected.is_none() {
                        if let Ok(procs) = &this.services {
                            this.selected = procs
                                .iter()
                                .find(|p| p.state == "healthy" && p.in_manifest)
                                .or_else(|| procs.iter().find(|p| p.state == "healthy"))
                                .map(|p| p.name.clone());
                        }
                    }
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            cx.background_executor().timer(POLL).await;
        })
        .detach();
    }

    /// Whether this workspace has an environment up at all. The pane uses it
    /// to decide whether to show the panel unasked — "x-ray into the sandbox
    /// when it's activated" reads as: when there is a sandbox, show it.
    /// What the daemon says about check running here.
    pub fn checks(&self) -> &preceipts_proto::ChecksState {
        &self.checks
    }

    /// Ask the daemon to run the checks.
    ///
    /// The window never runs them itself. A `cargo test` owned by a GPUI
    /// process dies or leaks when the window closes, and "processes must
    /// outlive the window" is the reason the daemon exists at all. So this is
    /// one socket call — the same one `preceipts run --detach` makes — and the
    /// answer arrives the way every answer arrives: as receipts, which the
    /// pane is already watching for.
    pub fn run_checks(&self, cx: &mut Context<Self>) {
        let socket = self.socket.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { daemon::call(&socket, &Request::Run { checks: None }) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(Response::Checks { state }) = result {
                    // Paint it as running now rather than up to a poll later:
                    // a button that looks inert for a second is a button
                    // people press twice.
                    this.checks = state;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn has_services(&self) -> bool {
        matches!(&self.services, Ok(procs) if !procs.is_empty())
    }

    fn select(&mut self, name: String, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(name.as_str()) {
            self.selected = None;
        } else {
            self.selected = Some(name);
            // A new service starts from the beginning of what the daemon still
            // holds, not from wherever the last one had got to.
            self.cursor = 0;
        }
        self.lines.clear();
        cx.notify();
    }

    /// What to paste into an agent so it knows which room it is standing in.
    ///
    /// Decision 12 says the app's job ends at the door, and this is the door:
    /// it does not run anything, it tells the agent what is already true and
    /// which verbs will tell it more. Built from live state on purpose — a
    /// brief listing services that are not up is worse than no brief.
    fn brief(&self) -> String {
        let mut out = String::new();
        out.push_str("You are working inside a preceipts workspace.\n\n");
        out.push_str(&format!("  worktree  {}\n", self.workspace.path.display()));
        if let Some(branch) = &self.workspace.branch {
            out.push_str(&format!("  branch    {branch}\n"));
        }
        if let Some(genesis) = &self.workspace.genesis {
            out.push_str(&format!("  intent    {genesis}\n"));
        }

        match &self.services {
            Ok(procs) if !procs.is_empty() => {
                out.push_str("\nAlready running — do not start your own copies:\n");
                for proc in procs {
                    let where_ = proc
                        .url
                        .clone()
                        .or_else(|| proc.port.map(|p| format!("http://localhost:{p}")))
                        .unwrap_or_else(|| "—".to_string());
                    out.push_str(&format!(
                        "  {:<14} {:<10} {where_}\n",
                        proc.name, proc.state
                    ));
                }
            }
            _ => {
                out.push_str("\nNo environment is up. `preceipts up` boots it.\n");
            }
        }

        out.push_str(
            "\nEverything this window shows, you can ask for yourself:\n\
             \x20 preceipts where                 which workspace this directory is\n\
             \x20 preceipts services              what is running, and is it healthy\n\
             \x20 preceipts proc <name> tail      a service's output\n\
             \x20 preceipts grep <pattern>        search every service at once\n\
             \x20 preceipts requests              every HTTP request the proxy carried\n\
             \x20 preceipts diff                  what has changed, against the base\n\
             \x20 preceipts run                   run the checks, mint receipts\n\
             \x20 preceipts run --detach          the same, handed to the daemon\n\
             \x20 preceipts checks                whether a run is happening now\n\
             \x20 preceipts status                the receipt table for this tree\n\
             \nWhile an environment is up here, the daemon runs the checks itself \n\
             whenever the worktree goes quiet — so `preceipts status` may already \n\
             have your answer before you ask for a run.\n\
             \nOr wire them in at once: `claude mcp add preceipts -- preceipts mcp`.\n",
        );
        out
    }

    fn render_services(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = match &self.services {
            Err(_) => Vec::new(),
            Ok(procs) => procs
                .iter()
                .map(|proc| {
                    let colour = match proc.state.as_str() {
                        "healthy" => cx.theme().success,
                        "crashed" | "killed" => cx.theme().danger,
                        "completed" => cx.theme().muted_foreground,
                        _ => cx.theme().warning,
                    };
                    let name = proc.name.clone();
                    let active = self.selected.as_deref() == Some(name.as_str());
                    // A port is a worse address than a hostname and a much
                    // better one than nothing: without the `:443` forwarder
                    // installed, the port is all there is.
                    let url = proc
                        .url
                        .clone()
                        .or_else(|| proc.port.map(|port| format!("http://localhost:{port}")));
                    v_flex()
                        .id(SharedString::from(proc.name.clone()))
                        .w_full()
                        .px_2()
                        .py_1()
                        .gap_0p5()
                        .rounded_sm()
                        .text_xs()
                        .when(active, |row| row.bg(cx.theme().sidebar_accent))
                        .hover(|row| row.bg(cx.theme().sidebar_accent))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().size(px(6.0)).flex_none().rounded_full().bg(colour))
                                .child(
                                    // `pkg#task` is the id, but the package
                                    // half is the same on every row of a
                                    // single-package project and is a whole
                                    // synthetic word (`preceipts#`) on a
                                    // manifest-only one. Dim it and let the
                                    // task name have the width.
                                    h_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when_some(
                                            proc.name.rsplit_once('#'),
                                            |row, (package, _)| {
                                                row.child(
                                                    div()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(SharedString::from(format!(
                                                            "{package}#"
                                                        ))),
                                                )
                                            },
                                        )
                                        .child(
                                            div().flex_none().child(SharedString::from(
                                                proc.name
                                                    .rsplit_once('#')
                                                    .map(|(_, task)| task.to_string())
                                                    .unwrap_or_else(|| proc.name.clone()),
                                            )),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(SharedString::from(proc.state.clone())),
                                ),
                        )
                        .when_some(url.clone(), |row, url| {
                            row.child(
                                div()
                                    .id(SharedString::from(format!("url-{}", proc.name)))
                                    .pl(px(14.0))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(cx.theme().muted_foreground)
                                    .hover(|s| s.text_color(cx.theme().foreground))
                                    .child(SharedString::from(url.clone()))
                                    // A URL you cannot click is a URL you
                                    // retype. This is the one place the panel
                                    // reaches outside the window, and it is
                                    // the same move as "Open in Editor".
                                    .on_click(move |_, _window, cx| cx.open_url(&url)),
                            )
                        })
                        .on_click(
                            cx.listener(move |this, _, _window, cx| this.select(name.clone(), cx)),
                        )
                })
                .collect(),
        };

        v_flex()
            .id("services")
            .flex_none()
            .max_h(px(260.0))
            .overflow_y_scroll()
            .px_1()
            .gap_0p5()
            .children(rows)
            .when(self.services.is_err(), |list| {
                list.child(
                    div()
                        .p_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        // Not an error. Most workspaces sit with no
                        // environment up most of the time, and saying "no
                        // daemon" in red would train people to ignore red.
                        .child("No environment up. `preceipts up` boots it."),
                )
            })
    }

    /// The lower half: one instrument at a time, with its own switch.
    fn render_instrument(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let font = self.font.clone();
        let showing = self.instrument;

        let tab = |label: &'static str, which: Instrument, cx: &mut Context<Self>| {
            div()
                .id(label)
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .text_xs()
                .when(showing == which, |t| {
                    t.bg(cx.theme().sidebar_accent)
                        .text_color(cx.theme().foreground)
                })
                .when(showing != which, |t| {
                    t.text_color(cx.theme().muted_foreground)
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.instrument = which;
                    cx.notify();
                }))
        };

        let body = match self.instrument {
            Instrument::Output => {
                let lines: Vec<_> = self
                    .lines
                    .iter()
                    .map(|line| {
                        div()
                            .w_full()
                            .whitespace_nowrap()
                            .child(SharedString::from(line.text.clone()))
                    })
                    .collect();
                v_flex()
                    .id("log")
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .overflow_scroll()
                    .track_scroll(&self.log_scroll)
                    .font_family(font)
                    .text_size(px(10.0))
                    .text_color(cx.theme().muted_foreground)
                    .children(lines)
                    .into_any_element()
            }
            Instrument::Requests => {
                // Newest last, the way a log reads. Status is the column worth
                // colouring: it is the one that says whether the code under
                // test answered or fell over.
                let rows: Vec<_> = self
                    .exchanges
                    .iter()
                    .map(|exchange| {
                        let (status, colour) = match exchange.status {
                            Some(code) if code < 300 => (code.to_string(), cx.theme().success),
                            Some(code) if code < 400 => {
                                (code.to_string(), cx.theme().muted_foreground)
                            }
                            Some(code) => (code.to_string(), cx.theme().danger),
                            // Recorded before the answer arrived, which is
                            // also exactly how a hung request looks.
                            None => ("…".to_string(), cx.theme().warning),
                        };
                        h_flex()
                            .w_full()
                            .gap_2()
                            .whitespace_nowrap()
                            .child(
                                div()
                                    .w(px(30.0))
                                    .flex_none()
                                    .text_color(colour)
                                    .child(SharedString::from(status)),
                            )
                            .child(
                                div()
                                    .w(px(34.0))
                                    .flex_none()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(exchange.method.clone())),
                            )
                            .child(
                                // The service, by its first label only. The
                                // rest of the name is the same on every row of
                                // one workspace and would push the path out.
                                div()
                                    .w(px(46.0))
                                    .flex_none()
                                    .overflow_hidden()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(
                                        exchange
                                            .host
                                            .split('.')
                                            .next()
                                            .unwrap_or(&exchange.host)
                                            .to_string(),
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(SharedString::from(exchange.path.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(
                                        exchange
                                            .duration_ms
                                            .map(|ms| format!("{ms}ms"))
                                            .unwrap_or_default(),
                                    )),
                            )
                    })
                    .collect();
                v_flex()
                    .id("requests")
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .overflow_scroll()
                    .font_family(font)
                    .text_size(px(10.0))
                    .children(rows)
                    .when(self.exchanges.is_empty(), |list| {
                        list.child(
                            div()
                                .p_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                // Says why it is empty rather than just being
                                // empty — and names the usual reason, which is
                                // not "nothing was served". Without the CA
                                // there is no proxy at all, the addresses above
                                // are direct ports, and traffic to them can
                                // never appear here however much of it there is.
                                .child(
                                    "Empty until the proxy is in the path. \
                                     `preceipts trust install`, then \
                                     `preceipts down && preceipts up`.",
                                ),
                        )
                    })
                    .into_any_element()
            }
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .px_1()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .child(tab("output", Instrument::Output, cx))
                    .child(tab("requests", Instrument::Requests, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(match self.instrument {
                                Instrument::Output => self
                                    .selected
                                    .clone()
                                    .unwrap_or_else(|| "pick a service".to_string()),
                                Instrument::Requests => {
                                    format!("{} exchanges", self.exchanges.len())
                                }
                            })),
                    ),
            )
            .child(body)
    }
}

impl Render for LabPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let copied = self.copied;
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .child("Lab")
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(self.workspace.id.clone())),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(
                                self.workspace.path.display().to_string(),
                            )),
                    ),
            )
            .child(Divider::horizontal())
            .child(self.render_services(cx))
            .child(Divider::horizontal())
            .child(self.render_instrument(cx))
            .child(Divider::horizontal())
            .child(
                h_flex().flex_none().p_1().child(
                    Button::new("brief")
                        .ghost()
                        .xsmall()
                        .label(if copied {
                            "copied — paste it to your agent"
                        } else {
                            "Copy agent brief"
                        })
                        .on_click(cx.listener(|this, _, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(this.brief()));
                            this.copied = true;
                            cx.notify();
                            // A confirmation, not a state. Left standing it
                            // becomes the button's name, and the next person
                            // to want the brief cannot tell whether pressing
                            // it did anything.
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_secs(3)).await;
                                let _ = this.update(cx, |this, cx| {
                                    this.copied = false;
                                    cx.notify();
                                });
                            })
                            .detach();
                        })),
                ),
            )
    }
}
