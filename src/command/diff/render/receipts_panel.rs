//! The receipts rail: a bottom-docked panel (above the footer) showing the
//! receipt table for the working tree, with live check runs streaming into
//! it. Overlays the diff like a drawer — the diff keeps scrolling behind it.

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::command::diff::receipts::{self, LiveState, RunSession};
use crate::command::diff::theme;

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

struct RowView {
    glyph: String,
    glyph_style: Style,
    check: String,
    /// "required" | "extra" | "prepare"
    tag: &'static str,
    state: String,
    state_style: Style,
    time: String,
    detail: String,
}

pub fn render_receipts_panel(
    frame: &mut Frame,
    area: Rect,
    session: Option<&RunSession>,
    message: Option<&str>,
) {
    let t = theme::get();
    let bg = t.ui.bg;
    let muted = Style::default().fg(t.ui.text_muted).bg(bg);
    let good = Style::default().fg(t.ui.stats_added).bg(bg);
    let bad = Style::default().fg(t.ui.stats_removed).bg(bg);
    let warn = Style::default().fg(t.ui.watching).bg(bg);
    let accent = Style::default().fg(t.ui.highlight).bg(bg);

    let status = receipts::status_get();
    let hud = receipts::get();

    // Merge the status table with the live session: prepare steps first (they
    // run first), then checks — a live entry wins over the (pre-run) receipt
    // state for that check.
    let mut rows: Vec<RowView> = Vec::new();
    let mut covered: Vec<&str> = Vec::new();
    if let Some(session) = session {
        for key in &session.order {
            if let Some(name) = key.strip_prefix("prepare:") {
                if let Some(live) = session.live.get(key) {
                    rows.push(live_row(name, "prepare", live, good, bad, accent));
                }
            }
        }
    }
    if let Some(ref status) = status {
        for row in &status.rows {
            covered.push(row.check.as_str());
            let live = session.and_then(|s| s.live.get(&row.check));
            rows.push(match live {
                Some(live) => live_row(
                    &row.check,
                    if row.required { "required" } else { "extra" },
                    live,
                    good,
                    bad,
                    accent,
                ),
                None => status_row(row, good, bad, warn, muted),
            });
        }
    }
    if let Some(session) = session {
        for key in &session.order {
            if key.starts_with("prepare:") || covered.contains(&key.as_str()) {
                continue;
            }
            if let Some(live) = session.live.get(key) {
                rows.push(live_row(key, "extra", live, good, bad, accent));
            }
        }
    }

    let content_lines = rows.len().max(1) as u16 + 1; // rows + hint line
    let height = (content_lines + 2).min(area.height.saturating_sub(4)); // + borders
    let y = area.height.saturating_sub(1 + height); // dock just above the footer
    let rect = Rect::new(0, y, area.width, height);
    frame.render_widget(Clear, rect);

    let title = match (&status, &hud) {
        (Some(status), hud) => {
            let dirty = hud.as_ref().map(|h| h.dirty).unwrap_or(false);
            format!(
                " receipts · tree {}{} · {} ",
                &status.tree[..status.tree.len().min(12)],
                if dirty { " (dirty)" } else { "" },
                if status.green { "green" } else { "not green" },
            )
        }
        _ => " receipts · no data (is .preceipts configured?) ".to_string(),
    };
    let title_style = if status.as_ref().map(|s| s.green).unwrap_or(false) {
        good
    } else {
        muted
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.ui.border_unfocused).bg(bg))
        .title(Line::styled(title, title_style))
        .style(Style::default().bg(bg));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let name_width = rows.iter().map(|r| r.check.len()).max().unwrap_or(5).max(5);
    let mut lines: Vec<Line> = Vec::new();
    for row in &rows {
        let mut spans = vec![
            Span::styled(format!(" {} ", row.glyph), row.glyph_style),
            Span::styled(
                format!("{:<width$}  ", row.check, width = name_width),
                Style::default().fg(t.ui.text_primary).bg(bg),
            ),
            Span::styled(format!("{:<9}", row.tag), muted),
            Span::styled(format!("{:<17}", row.state), row.state_style),
            Span::styled(format!("{:>8}  ", row.time), muted),
        ];
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let room = (inner.width as usize).saturating_sub(used + 1);
        let mut detail = row.detail.clone();
        if detail.chars().count() > room {
            detail = detail
                .chars()
                .take(room.saturating_sub(1))
                .collect::<String>()
                + "…";
        }
        spans.push(Span::styled(detail, muted));
        lines.push(Line::from(spans));
    }
    if rows.is_empty() {
        lines.push(Line::styled(
            " no checks configured — add executables under .preceipts/checks/",
            muted,
        ));
    }

    // Hint / message line: explicit messages (land results) win, then the
    // run session's note (normalized / invalidated), then key hints.
    let running = session.map(|s| s.is_running()).unwrap_or(false);
    let message = message.or_else(|| session.and_then(|s| s.note.as_deref()));
    let hint = if let Some(message) = message {
        Line::from(vec![Span::styled(format!(" {message}"), warn)])
    } else if running {
        Line::from(vec![Span::styled(" running… · R close", muted)])
    } else {
        Line::from(vec![Span::styled(
            " r run checks · t scope · R close",
            muted,
        )])
    };
    lines.push(hint);

    frame.render_widget(Paragraph::new(lines).style(Style::default().bg(bg)), inner);
}

fn live_row(
    check: &str,
    tag: &'static str,
    live: &receipts::LiveCheck,
    good: Style,
    bad: Style,
    accent: Style,
) -> RowView {
    match live.state {
        LiveState::Running => {
            let elapsed = live.started_at.elapsed();
            let frame_index = (elapsed.as_millis() / 120) as usize % SPINNER.len();
            RowView {
                glyph: SPINNER[frame_index].to_string(),
                glyph_style: accent,
                check: check.to_string(),
                tag,
                state: "running".to_string(),
                state_style: accent,
                time: receipts::format_ms(elapsed.as_millis() as u64),
                detail: live.last_line.clone(),
            }
        }
        LiveState::Passed => RowView {
            glyph: "✓".to_string(),
            glyph_style: good,
            check: check.to_string(),
            tag,
            state: "ok".to_string(),
            state_style: good,
            time: live
                .duration_ms
                .map(receipts::format_ms)
                .unwrap_or_default(),
            detail: live.last_line.clone(),
        },
        LiveState::Failed => RowView {
            glyph: "✗".to_string(),
            glyph_style: bad,
            check: check.to_string(),
            tag,
            state: "fail".to_string(),
            state_style: bad,
            time: live
                .duration_ms
                .map(receipts::format_ms)
                .unwrap_or_default(),
            detail: live.last_line.clone(),
        },
    }
}

fn status_row(
    row: &receipts::StatusRow,
    good: Style,
    bad: Style,
    warn: Style,
    muted: Style,
) -> RowView {
    let (glyph, style) = match row.state.as_str() {
        "ok" => ("✓", good),
        "fail" => ("✗", bad),
        "stale-definition" => ("≠", warn),
        _ => ("∅", muted),
    };
    let detail = row
        .receipt
        .as_ref()
        .map(|receipt| {
            let who = if receipt.runner.email.is_empty() {
                receipt.runner.name.clone()
            } else {
                receipt.runner.email.clone()
            };
            let agent = receipt
                .runner
                .agent
                .as_deref()
                .map(|a| format!(" ({})", a.split('/').next().unwrap_or(a)))
                .unwrap_or_default();
            format!("{} · {}{}", receipt.started, who, agent)
        })
        .unwrap_or_else(|| match row.state.as_str() {
            "stale-definition" => "check script changed since this receipt".to_string(),
            _ => "no receipt for this tree".to_string(),
        });
    RowView {
        glyph: glyph.to_string(),
        glyph_style: style,
        check: row.check.clone(),
        tag: if row.required { "required" } else { "extra" },
        state: row.state.clone(),
        state_style: style,
        time: row
            .receipt
            .as_ref()
            .map(|r| receipts::format_ms(r.duration_ms))
            .unwrap_or_default(),
        detail,
    }
}
