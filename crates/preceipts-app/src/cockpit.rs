//! The cockpit layout: workspace list, diff surface, status HUD.
//!
//! The shape docs/direction-2026-08.md asks for, minus the tabs and env panel
//! that need the daemon. Glanceable is a hard requirement here rather than a
//! nicety — this window is meant to sit *beside* the agent conversation at half
//! screen width, not to be looked at full-screen.

use crate::surface_view::SurfaceView;
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, FontWeight, SharedString, Window};
use preceipts_core::checks::{CheckState, Status};
use preceipts_core::workspace::Workspace;
use preceipts_core::Changeset;

pub struct Cockpit {
    workspaces: Vec<Workspace>,
    /// Which workspace this window is showing.
    current: String,
    changeset_summary: (usize, usize, usize, String),
    status: Option<Status>,
    surface: Entity<SurfaceView>,
    theme: Theme,
}

impl Cockpit {
    pub fn new(
        changeset: Changeset,
        workspaces: Vec<Workspace>,
        current: String,
        status: Option<Status>,
        cx: &mut Context<Self>,
    ) -> Self {
        let summary = (
            changeset.files.len(),
            changeset.total_added(),
            changeset.total_removed(),
            changeset.base_name.clone(),
        );
        let surface = cx.new(|_| SurfaceView::new(changeset));
        Self {
            workspaces,
            current,
            changeset_summary: summary,
            status,
            surface,
            theme: Theme::default(),
        }
    }

    fn render_workspace_row(&self, workspace: &Workspace) -> gpui::AnyElement {
        let selected = workspace.id == self.current;
        let branch = workspace
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".to_string());

        let mut row = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_l_2()
            .child(
                div()
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(self.theme.text)
                    .child(SharedString::from(branch)),
            );

        // The recorded intent is what makes eight worktrees legible next week,
        // so it is on the card rather than behind a hover.
        if let Some(genesis) = &workspace.genesis {
            row = row.child(
                div()
                    .text_size(px(11.0))
                    .text_color(self.theme.text_dim)
                    .child(SharedString::from(genesis.clone())),
            );
        }

        row.border_color(if selected {
            self.theme.text
        } else {
            self.theme.surface
        })
        .bg(if selected {
            self.theme.header_bg
        } else {
            self.theme.surface
        })
        .into_any_element()
    }

    fn render_sidebar(&self) -> gpui::AnyElement {
        let mut sidebar = div()
            .w(px(240.0))
            .flex_none()
            .flex()
            .flex_col()
            .bg(self.theme.surface)
            .border_r_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(self.theme.text_dim)
                    .child(SharedString::from(format!(
                        "{} workspace{}",
                        self.workspaces.len(),
                        if self.workspaces.len() == 1 { "" } else { "s" }
                    ))),
            );
        for workspace in &self.workspaces {
            sidebar = sidebar.child(self.render_workspace_row(workspace));
        }
        sidebar.into_any_element()
    }

    /// The footer: diff stats, and the receipt verdict.
    fn render_hud(&self) -> gpui::AnyElement {
        let (files, added, removed, base) = &self.changeset_summary;

        let (verdict, verdict_color) = match &self.status {
            None => ("no checks".to_string(), self.theme.text_dim),
            Some(status) if status.green => ("green".to_string(), self.theme.added_emphasis),
            Some(status) => {
                let failing: Vec<&str> = status
                    .rows
                    .iter()
                    .filter(|row| row.required && row.state != CheckState::Ok)
                    .map(|row| row.check.as_str())
                    .collect();
                (failing.join(", "), self.theme.removed_emphasis)
            }
        };

        div()
            .h(px(26.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .bg(self.theme.header_bg)
            .border_t_1()
            .border_color(self.theme.border)
            .text_size(px(11.0))
            .child(
                div()
                    .text_color(self.theme.text_dim)
                    .child(SharedString::from(format!("vs {base}"))),
            )
            .child(
                div()
                    .text_color(self.theme.text_dim)
                    .child(SharedString::from(format!("{files} files"))),
            )
            .child(
                div()
                    .text_color(self.theme.text)
                    .child(SharedString::from(format!("+{added} −{removed}"))),
            )
            .child(
                div()
                    .px_2()
                    .bg(verdict_color)
                    .text_color(self.theme.text)
                    .child(SharedString::from(verdict)),
            )
            .into_any_element()
    }
}

impl Render for Cockpit {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.background)
            .text_color(self.theme.text)
            .font_family("SF Mono")
            .text_size(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar())
                    .child(div().flex_1().child(self.surface.clone())),
            )
            .child(self.render_hud())
    }
}
