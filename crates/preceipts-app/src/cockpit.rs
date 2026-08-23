//! The window: one project, a tab per workspace.
//!
//! One window per project, and the tabs across the top are its worktrees. That
//! amends the UI-shape section of docs/direction-2026-08.md, which had it the
//! other way round — tabs for projects, workspaces in a list down the side. A
//! window is the unit you arrange on a screen beside an agent, and what you
//! arrange beside an agent is a project; the workspaces are what you flip
//! between while it works.
//!
//! The `+` is the third door of decision 12 made visible. It does not create
//! anything: it lists the worktrees that already exist and are not open yet.
//! `.git/worktrees/` is watched, so a worktree an agent creates in a terminal
//! appears in that list within a moment of git writing it, newest at the top —
//! click `+`, ask the agent for a branch, and watch it arrive.

use crate::workspace_pane::{Activity, Verdict, WorkspacePane};
use futures::StreamExt as _;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, SharedString, Window};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::popover::Popover;
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, TitleBar};
use preceipts_core::workspace::{self, Workspace};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// An open tab. The pane is a `Result` because one unreadable worktree must
/// not be able to take the window down — before tabs, a failed load was a
/// process exit, which is a different thing entirely once there are five of
/// them.
struct TabState {
    workspace: Workspace,
    pane: Result<Entity<WorkspacePane>, String>,
    /// The worktree was removed while this tab was open. The tab stays —
    /// closing someone's view out from under them is worse than a dead one —
    /// but it stops claiming to be a live workspace.
    gone: bool,
}

pub struct Cockpit {
    project_root: PathBuf,
    project_name: SharedString,
    tabs: Vec<TabState>,
    active: usize,
    /// Every workspace of this project, newest first.
    known: Vec<Workspace>,
    /// Ids that were not there when the window opened. These are the ones an
    /// agent just made, which is exactly the case the `+` list exists for.
    fresh: HashSet<String>,
}

impl Cockpit {
    pub fn new(
        project_root: PathBuf,
        project_name: String,
        first: Workspace,
        cx: &mut Context<Self>,
    ) -> Self {
        let known = Self::ordered(&project_root);
        let mut this = Self {
            project_root: project_root.clone(),
            project_name: project_name.into(),
            tabs: Vec::new(),
            active: 0,
            known,
            fresh: HashSet::new(),
        };
        this.open_workspace(first, cx);
        this.watch(cx);
        this
    }

    /// Discovered workspaces, newest first.
    ///
    /// Birth time rather than branch name or discovery order: what a person
    /// wants at the top of the list is the worktree that just appeared, and
    /// the primary working directory — always the oldest — settles to the
    /// bottom on its own without a special case.
    fn ordered(project_root: &Path) -> Vec<Workspace> {
        let mut all = workspace::discover(project_root).unwrap_or_default();
        all.sort_by_key(|w| {
            std::cmp::Reverse(workspace::created_at(w).unwrap_or(SystemTime::UNIX_EPOCH))
        });
        all
    }

    /// Door three: watch `.git/worktrees/` and adopt what shows up.
    ///
    /// The watch blocks, so it lives on its own thread and reaches the window
    /// through a channel. It sends no payload — [`workspace::discover`] is the
    /// only thing that knows what a workspace is, and it is cheap enough to
    /// re-run.
    fn watch(&self, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let root = self.project_root.clone();
        std::thread::spawn(move || {
            let _ = workspace::watch_worktrees(&root, || tx.unbounded_send(()).is_ok());
        });
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                if this.update(cx, |this, cx| this.rediscover(cx)).is_err() {
                    // The window went away; so does the watch.
                    break;
                }
            }
        })
        .detach();
    }

    fn rediscover(&mut self, cx: &mut Context<Self>) {
        let before: HashSet<String> = self.known.iter().map(|w| w.id.clone()).collect();
        self.known = Self::ordered(&self.project_root);
        for workspace in &self.known {
            if !before.contains(&workspace.id) {
                self.fresh.insert(workspace.id.clone());
            }
        }
        // The same watch that notices a worktree appearing notices one going
        // away, and an open tab is the only place that fact would otherwise
        // not reach — rediscovery fixes the `+` list on its own.
        let live: HashSet<&str> = self.known.iter().map(|w| w.id.as_str()).collect();
        for tab in &mut self.tabs {
            tab.gone = !live.contains(tab.workspace.id.as_str());
        }
        cx.notify();
    }

    /// Open a workspace as a tab, or focus it if it is already open.
    fn open_workspace(&mut self, workspace: Workspace, cx: &mut Context<Self>) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.workspace.id == workspace.id)
        {
            self.active = index;
            return;
        }
        self.fresh.remove(&workspace.id);
        let pane = WorkspacePane::open(workspace.clone(), cx);
        // The tab's dot is the pane's verdict, and a pane updates itself when
        // the tree goes quiet. Without this the badge would only refresh when
        // something else happened to repaint the window — which, for a person
        // watching an agent work, is never.
        if let Ok(pane) = &pane {
            cx.observe(pane, |_, _, cx| cx.notify()).detach();
        }
        self.tabs.push(TabState {
            workspace,
            pane,
            gone: false,
        });
        self.active = self.tabs.len() - 1;
    }

    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            // The last tab stays: a window with no workspace in it is a window
            // with nothing to say, and closing the window is what that means.
            return;
        }
        self.tabs.remove(index);
        self.active = self.active.min(self.tabs.len() - 1);
    }

    fn label(workspace: &Workspace) -> SharedString {
        workspace
            .branch
            .clone()
            .unwrap_or_else(|| workspace.id.clone())
            .into()
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open: HashSet<String> = self.tabs.iter().map(|t| t.workspace.id.clone()).collect();
        // Newest first, and already-open workspaces are not offered again —
        // clicking `+` should never be a no-op that looks like a bug.
        let candidates: Vec<(Workspace, bool)> = self
            .known
            .iter()
            .filter(|w| !open.contains(&w.id))
            .map(|w| (w.clone(), self.fresh.contains(&w.id)))
            .collect();

        let closable = self.tabs.len() > 1;
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                // The badge — step 8's other half. It says what the receipts
                // say, which is the only thing worth a tab's one pixel of
                // colour: primary-versus-linked, which it used to show, is
                // already in the label and never changes.
                let dot = match (&tab.pane, tab.gone) {
                    (Err(_), _) => cx.theme().danger,
                    (_, true) => cx.theme().border,
                    (Ok(pane), false) => {
                        let pane = pane.read(cx);
                        // Amber outranks the verdict: while the tree is
                        // changing, the last verdict is about a tree that no
                        // longer exists, and showing it green would be a lie
                        // with a specific cost.
                        if pane.activity() == Activity::Working {
                            cx.theme().warning
                        } else {
                            match pane.verdict() {
                                Verdict::Green => cx.theme().success,
                                Verdict::Failed => cx.theme().danger,
                                Verdict::NotRun => cx.theme().muted_foreground,
                                Verdict::Absent => cx.theme().border,
                            }
                        }
                    }
                };
                Tab::new()
                    .label(Self::label(&tab.workspace))
                    // A dot rather than an icon: `IconName` needs an
                    // `AssetSource` this app does not install, and what the
                    // dot has to say is one colour anyway.
                    .prefix(div().size(px(6.0)).rounded_full().bg(dot).flex_none())
                    .when(closable, |tab| {
                        tab.suffix(
                            Button::new(("close-tab", index))
                                .ghost()
                                .xsmall()
                                .label("×")
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.close_tab(index);
                                    cx.notify();
                                })),
                        )
                    })
            })
            .collect();

        TabBar::new("workspaces")
            .underline()
            .small()
            .selected_index(self.active)
            .on_click(cx.listener(|this, index: &usize, _window, cx| {
                this.active = *index;
                cx.notify();
            }))
            .children(tabs)
            .suffix(self.render_plus(candidates, cx))
    }

    /// The `+`: everything this project already has that is not on screen.
    fn render_plus(
        &self,
        candidates: Vec<(Workspace, bool)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let waiting = candidates.len();
        let arrived = candidates.iter().any(|(_, fresh)| *fresh);
        // The count rides on the button itself. "Click `+` and wait for the
        // agent to make one" only works if arrival is visible without the menu
        // open — a worktree that appeared behind a closed popover is a worktree
        // you never learn about.
        let trigger = Button::new("plus")
            .ghost()
            .xsmall()
            .label(if waiting == 0 {
                "+".to_string()
            } else {
                format!("+ {waiting}")
            })
            // Green means one of them showed up while you were reading
            // something else. The popover's trigger has to be `Selectable`,
            // so the signal is the button's own colour rather than a dot
            // beside it.
            .when(arrived, |button| button.text_color(cx.theme().success));
        Popover::new("open-workspace")
            .trigger(trigger)
            .content(move |_state, _window, cx| {
                let this = this.clone();
                let rows: Vec<_> = candidates
                    .iter()
                    .map(|(workspace, fresh)| {
                        let (workspace, fresh) = (workspace.clone(), *fresh);
                        let this = this.clone();
                        let label = Self::label(&workspace);
                        let genesis = workspace.genesis.clone();
                        h_flex()
                            .id(SharedString::from(workspace.id.clone()))
                            .w(px(320.0))
                            .px_2()
                            .py_1()
                            .gap_2()
                            .rounded_sm()
                            .text_xs()
                            .hover(|s| s.bg(cx.theme().sidebar_accent))
                            .child(
                                div()
                                    .size(px(6.0))
                                    .flex_none()
                                    .rounded_full()
                                    // A dot only on what arrived while you
                                    // were looking at something else.
                                    .bg(if fresh {
                                        cx.theme().success
                                    } else {
                                        cx.theme().border
                                    }),
                            )
                            .child(div().flex_none().child(label))
                            .when_some(genesis, |row, genesis| {
                                // The recorded intent, which is the thing that
                                // makes eight worktrees legible next week.
                                row.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(SharedString::from(genesis)),
                                )
                            })
                            .on_click({
                                let workspace = workspace.clone();
                                move |_, _window, cx| {
                                    let workspace = workspace.clone();
                                    let _ = this.update(cx, |cockpit, cx| {
                                        cockpit.open_workspace(workspace, cx);
                                        cx.notify();
                                    });
                                }
                            })
                    })
                    .collect();

                v_flex()
                    .p_1()
                    .gap_0p5()
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .w(px(320.0))
                                .p_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                // Deliberately not a "New workspace" button. The
                                // app is a registry that notices, not a factory
                                // that must be used — so the empty state names the
                                // two doors that do create, and waits.
                                .child(
                                    "Every worktree is open. \
                                 `preceipts new \"…\"`, or ask your agent for one — \
                                 it shows up here.",
                                ),
                        )
                    })
                    .children(rows)
            })
    }
}

impl Render for Cockpit {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tabs.get(self.active) {
            Some(TabState { pane: Ok(pane), .. }) => pane.clone().into_any_element(),
            Some(TabState {
                pane: Err(message),
                workspace,
                ..
            }) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .child(SharedString::from(workspace.path.display().to_string()))
                .child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(message.clone())),
                )
                .into_any_element(),
            None => div().into_any_element(),
        };

        v_flex()
            .size_full()
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_center()
                        .text_sm()
                        .child(self.project_name.clone())
                        .child(self.render_tab_bar(cx))
                        // Right edge: what the active tab amounts to, so the
                        // window says something useful while the diff itself
                        // is scrolled somewhere in the middle.
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(
                                    self.tabs
                                        .get(self.active)
                                        .and_then(|tab| tab.pane.as_ref().ok())
                                        .map(|pane| pane.read(cx).summary())
                                        .unwrap_or_default(),
                                )),
                        ),
                ),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
    }
}
