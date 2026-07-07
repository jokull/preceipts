//! The Cockpit entity: all app state and behavior. Rendering lives in
//! render.rs; pure segment math in segments.rs.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use gpui::{actions, Context, UniformListScrollHandle, Window};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, Debouncer};
use preceipts_core::{
    Changeset, CommentStore, CoreError, DiffScope, FileHighlight, LocalComment, PrFeedback, Side,
};

actions!(
    preceipts,
    [
        Quit,
        ToggleScope,
        FindOpen,
        FindNext,
        FindPrev,
        Dismiss,
        ToggleFeedback,
        ComposeComment,
        CopyAllFeedback
    ]
);

/// One entry in the flattened scroll surface. Fixed-height by construction
/// (the load-bearing decision from docs/desktop-foundations.md).
pub enum SurfaceRow {
    FileHeader { file: usize },
    Gap { skipped: u32 },
    Line { file: usize, hunk: usize, row: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    None,
    Find,
    Compose,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FeedbackFilter {
    All,
    Humans,
    Bots,
}

impl FeedbackFilter {
    pub fn next(self) -> FeedbackFilter {
        match self {
            FeedbackFilter::All => FeedbackFilter::Humans,
            FeedbackFilter::Humans => FeedbackFilter::Bots,
            FeedbackFilter::Bots => FeedbackFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FeedbackFilter::All => "all",
            FeedbackFilter::Humans => "humans",
            FeedbackFilter::Bots => "bots",
        }
    }
}

pub struct Cockpit {
    pub repo: PathBuf,
    pub scope: DiffScope,
    pub changeset: Option<Changeset>,
    pub error: Option<String>,
    pub loading: bool,
    pub surface: Vec<SurfaceRow>,
    /// surface index of each file's header, aligned with changeset.files.
    pub file_anchors: Vec<usize>,
    pub scroll: UniformListScrollHandle,
    /// Lazy per-(file, old-side) syntax highlight cache; None = unsupported.
    pub highlights: HashMap<(usize, bool), Option<FileHighlight>>,

    pub input_mode: InputMode,
    pub find_query: String,
    pub find_matches: Vec<usize>,
    pub find_current: usize,

    /// Clicked diff row (surface index) — anchor for composed comments.
    pub selected_row: Option<usize>,
    pub compose_text: String,
    pub drafts: Vec<LocalComment>,
    /// (file index, 1-based line, is_old_side) rows that carry a draft.
    pub draft_anchors: HashSet<(usize, u32, bool)>,

    pub feedback_open: bool,
    pub feedback: Option<PrFeedback>,
    pub feedback_loading: bool,
    pub feedback_error: Option<String>,
    pub feedback_filter: FeedbackFilter,

    reload_in_flight: bool,
    reload_queued: bool,
    _watcher: Option<Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>>,
}

impl Cockpit {
    pub fn new(repo: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut cockpit = Cockpit {
            repo,
            scope: DiffScope::Branch,
            changeset: None,
            error: None,
            loading: false,
            surface: Vec::new(),
            file_anchors: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            highlights: HashMap::new(),
            input_mode: InputMode::None,
            find_query: String::new(),
            find_matches: Vec::new(),
            find_current: 0,
            selected_row: None,
            compose_text: String::new(),
            drafts: Vec::new(),
            draft_anchors: HashSet::new(),
            feedback_open: false,
            feedback: None,
            feedback_loading: false,
            feedback_error: None,
            feedback_filter: FeedbackFilter::All,
            reload_in_flight: false,
            reload_queued: false,
            _watcher: None,
        };
        cockpit.request_reload(cx);
        cockpit
    }

    // ------------------------------------------------------------------
    // Loading & watching

    /// Single-flight background reload (the TUI process-storm lesson):
    /// requests during a flight coalesce into exactly one follow-up.
    pub fn request_reload(&mut self, cx: &mut Context<Self>) {
        if self.reload_in_flight {
            self.reload_queued = true;
            return;
        }
        self.reload_in_flight = true;
        self.loading = self.changeset.is_none();
        let repo = self.repo.clone();
        let scope = self.scope;
        cx.spawn(async move |this, cx| {
            let task = cx
                .background_executor()
                .spawn(async move { Changeset::load(&repo, scope) });
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.reload_in_flight = false;
                this.loading = false;
                this.apply_reload(result, cx);
                if this.reload_queued {
                    this.reload_queued = false;
                    this.request_reload(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_reload(&mut self, result: Result<Changeset, CoreError>, cx: &mut Context<Self>) {
        match result {
            Ok(changeset) => {
                self.error = None;
                self.rebuild_surface(&changeset);
                self.highlights.clear();
                self.ensure_watcher(&changeset, cx);
                self.changeset = Some(changeset);
                self.reload_drafts();
                self.update_find_matches();
            }
            Err(err) => {
                self.error = Some(err.to_string());
            }
        }
    }

    fn rebuild_surface(&mut self, changeset: &Changeset) {
        self.surface.clear();
        self.file_anchors.clear();
        self.selected_row = None;
        for (file_idx, file) in changeset.files.iter().enumerate() {
            self.file_anchors.push(self.surface.len());
            self.surface.push(SurfaceRow::FileHeader { file: file_idx });
            for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
                if hunk.skipped_before > 0 {
                    self.surface.push(SurfaceRow::Gap {
                        skipped: hunk.skipped_before,
                    });
                }
                for row_idx in 0..hunk.rows.len() {
                    self.surface.push(SurfaceRow::Line {
                        file: file_idx,
                        hunk: hunk_idx,
                        row: row_idx,
                    });
                }
            }
        }
    }

    fn ensure_watcher(&mut self, changeset: &Changeset, cx: &mut Context<Self>) {
        if self._watcher.is_some() {
            return;
        }
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let workdir = changeset.workdir.clone();
        let mut debouncer = match new_debouncer(
            Duration::from_millis(400),
            move |events: notify_debouncer_mini::DebounceEventResult| {
            if let Ok(events) = events {
                let noise = [".git", "target", "node_modules", ".turbo", "dist", ".next"];
                let relevant = |path: &std::path::Path| {
                    !path.components().any(|c| {
                        noise
                            .iter()
                            .any(|n| c.as_os_str().to_string_lossy() == *n)
                    })
                };
                if events
                    .iter()
                    .any(|e: &notify_debouncer_mini::DebouncedEvent| relevant(&e.path))
                {
                    let _ = tx.unbounded_send(());
                }
            }
            },
        ) {
            Ok(debouncer) => debouncer,
            Err(_) => return, // no watching; manual reloads still work
        };
        if debouncer
            .watcher()
            .watch(&workdir, RecursiveMode::Recursive)
            .is_err()
        {
            return;
        }
        self._watcher = Some(debouncer);
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                if this
                    .update(cx, |this, cx| this.request_reload(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Actions

    pub fn toggle_scope(&mut self, _: &ToggleScope, _window: &mut Window, cx: &mut Context<Self>) {
        self.scope = match self.scope {
            DiffScope::Branch => DiffScope::Uncommitted,
            DiffScope::Uncommitted => DiffScope::Branch,
        };
        self.request_reload(cx);
        cx.notify();
    }

    pub fn find_open(&mut self, _: &FindOpen, _window: &mut Window, cx: &mut Context<Self>) {
        self.input_mode = InputMode::Find;
        cx.notify();
    }

    pub fn dismiss(&mut self, _: &Dismiss, _window: &mut Window, cx: &mut Context<Self>) {
        match self.input_mode {
            InputMode::Compose => {
                self.compose_text.clear();
                self.input_mode = InputMode::None;
            }
            InputMode::Find => {
                self.input_mode = InputMode::None;
                self.find_query.clear();
                self.find_matches.clear();
            }
            InputMode::None => {
                self.feedback_open = false;
            }
        }
        cx.notify();
    }

    pub fn find_next(&mut self, _: &FindNext, _window: &mut Window, cx: &mut Context<Self>) {
        self.step_match(1);
        cx.notify();
    }

    pub fn find_prev(&mut self, _: &FindPrev, _window: &mut Window, cx: &mut Context<Self>) {
        self.step_match(-1);
        cx.notify();
    }

    pub fn toggle_feedback(
        &mut self,
        _: &ToggleFeedback,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.feedback_open = !self.feedback_open;
        if self.feedback_open && self.feedback.is_none() && !self.feedback_loading {
            self.fetch_feedback(cx);
        }
        cx.notify();
    }

    pub fn compose_comment(
        &mut self,
        _: &ComposeComment,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_line_anchor().is_some() {
            self.input_mode = InputMode::Compose;
            cx.notify();
        }
    }

    pub fn copy_all_feedback(
        &mut self,
        _: &CopyAllFeedback,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let digest = self.combined_digest();
        if !digest.trim().is_empty() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(digest));
        }
    }

    /// Typed input routed to whichever bar is open (find or compose).
    /// Plain key events only — command chords stay with the keymap.
    pub fn handle_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_mode == InputMode::None || event.keystroke.modifiers.platform {
            return;
        }
        let target_is_find = self.input_mode == InputMode::Find;
        match event.keystroke.key.as_str() {
            "backspace" => {
                if target_is_find {
                    self.find_query.pop();
                    self.update_find_matches();
                } else {
                    self.compose_text.pop();
                }
            }
            "enter" => {
                if target_is_find {
                    self.step_match(1);
                } else {
                    self.save_comment();
                }
            }
            "escape" => return, // Dismiss action handles it
            _ => {
                if let Some(ch) = &event.keystroke.key_char {
                    if target_is_find {
                        self.find_query.push_str(ch);
                        self.update_find_matches();
                    } else {
                        self.compose_text.push_str(ch);
                    }
                }
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Find

    pub fn update_find_matches(&mut self) {
        self.find_matches.clear();
        self.find_current = 0;
        let query = self.find_query.to_lowercase();
        if query.is_empty() {
            return;
        }
        let Some(changeset) = &self.changeset else {
            return;
        };
        for (ix, row) in self.surface.iter().enumerate() {
            let hit = match row {
                SurfaceRow::FileHeader { file } => {
                    changeset.files[*file].path.to_lowercase().contains(&query)
                }
                SurfaceRow::Gap { .. } => false,
                SurfaceRow::Line { file, hunk, row } => {
                    let r = &changeset.files[*file].hunks[*hunk].rows[*row];
                    [r.old.as_ref(), r.new.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|line| line.text.to_lowercase().contains(&query))
                }
            };
            if hit {
                self.find_matches.push(ix);
            }
        }
        if let Some(first) = self.find_matches.first() {
            self.scroll
                .scroll_to_item(*first, gpui::ScrollStrategy::Center);
        }
    }

    fn step_match(&mut self, delta: isize) {
        if self.find_matches.is_empty() {
            return;
        }
        let len = self.find_matches.len() as isize;
        self.find_current = ((self.find_current as isize + delta).rem_euclid(len)) as usize;
        self.scroll.scroll_to_item(
            self.find_matches[self.find_current],
            gpui::ScrollStrategy::Center,
        );
    }

    // ------------------------------------------------------------------
    // Comments (local drafts)

    /// The (path, line, side, line_text) a composed comment would anchor to:
    /// the selected row's new side when present, else its old side.
    pub fn selected_line_anchor(&self) -> Option<(String, u32, Side, String)> {
        let changeset = self.changeset.as_ref()?;
        let ix = self.selected_row?;
        let SurfaceRow::Line { file, hunk, row } = self.surface.get(ix)? else {
            return None;
        };
        let file_diff = &changeset.files[*file];
        let r = &file_diff.hunks[*hunk].rows[*row];
        if let Some(new) = &r.new {
            Some((file_diff.path.clone(), new.number, Side::New, new.text.clone()))
        } else {
            r.old
                .as_ref()
                .map(|old| (file_diff.path.clone(), old.number, Side::Old, old.text.clone()))
        }
    }

    fn save_comment(&mut self) {
        let Some((path, line, side, line_text)) = self.selected_line_anchor() else {
            return;
        };
        let body = self.compose_text.trim().to_string();
        if body.is_empty() {
            return;
        }
        let Some(changeset) = &self.changeset else {
            return;
        };
        let branch = changeset.info.branch.clone().unwrap_or_default();
        if let Ok(mut store) = CommentStore::open(&changeset.git_dir, &branch) {
            let _ = store.add(&path, line, side, &line_text, &body);
        }
        self.compose_text.clear();
        self.input_mode = InputMode::None;
        self.reload_drafts();
    }

    pub fn remove_draft(&mut self, id: u64) {
        let Some(changeset) = &self.changeset else {
            return;
        };
        let branch = changeset.info.branch.clone().unwrap_or_default();
        if let Ok(mut store) = CommentStore::open(&changeset.git_dir, &branch) {
            let _ = store.remove(id);
        }
        self.reload_drafts();
    }

    pub fn reload_drafts(&mut self) {
        self.drafts.clear();
        self.draft_anchors.clear();
        let Some(changeset) = &self.changeset else {
            return;
        };
        let branch = changeset.info.branch.clone().unwrap_or_default();
        let Ok(store) = CommentStore::open(&changeset.git_dir, &branch) else {
            return;
        };
        self.drafts = store.comments().to_vec();
        for draft in &self.drafts {
            if let Some(file_idx) = changeset.files.iter().position(|f| f.path == draft.path) {
                self.draft_anchors
                    .insert((file_idx, draft.line, draft.side == Side::Old));
            }
        }
    }

    pub fn drafts_digest(&self) -> String {
        let Some(changeset) = &self.changeset else {
            return String::new();
        };
        let branch = changeset.info.branch.clone().unwrap_or_default();
        CommentStore::open(&changeset.git_dir, &branch)
            .map(|store| store.digest())
            .unwrap_or_default()
    }

    /// Drafts + currently-filtered GitHub feedback, one paste-ready digest.
    pub fn combined_digest(&self) -> String {
        let mut out = String::new();
        let drafts = self.drafts_digest();
        if !drafts.trim().is_empty() {
            out.push_str("# Draft comments\n\n");
            out.push_str(&drafts);
        }
        if let Some(feedback) = &self.feedback {
            let filtered: Vec<_> = feedback
                .comments
                .iter()
                .filter(|c| self.feedback_filter_allows(c.is_bot))
                .collect();
            if !filtered.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!(
                    "# PR #{} feedback ({})\n\n",
                    feedback.number,
                    self.feedback_filter.label()
                ));
                out.push_str(&PrFeedback::digest(filtered));
            }
        }
        out
    }

    pub fn feedback_filter_allows(&self, is_bot: bool) -> bool {
        match self.feedback_filter {
            FeedbackFilter::All => true,
            FeedbackFilter::Humans => !is_bot,
            FeedbackFilter::Bots => is_bot,
        }
    }

    // ------------------------------------------------------------------
    // GitHub feedback

    pub fn fetch_feedback(&mut self, cx: &mut Context<Self>) {
        let Some(changeset) = &self.changeset else {
            return;
        };
        self.feedback_loading = true;
        self.feedback_error = None;
        let workdir = changeset.workdir.clone();
        cx.spawn(async move |this, cx| {
            let task = cx
                .background_executor()
                .spawn(async move { preceipts_core::fetch_pr_feedback(&workdir) });
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.feedback_loading = false;
                match result {
                    Ok(feedback) => this.feedback = Some(feedback),
                    Err(err) => this.feedback_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Scroll the surface to a feedback comment's anchor, if it maps to a
    /// row in the current diff.
    pub fn reveal_anchor(&mut self, path: &str, line: u32) {
        let Some(changeset) = &self.changeset else {
            return;
        };
        let Some(file_idx) = changeset.files.iter().position(|f| f.path == path) else {
            return;
        };
        for (ix, row) in self.surface.iter().enumerate() {
            if let SurfaceRow::Line { file, hunk, row } = row {
                if *file != file_idx {
                    continue;
                }
                let r = &changeset.files[*file].hunks[*hunk].rows[*row];
                if r.new.as_ref().is_some_and(|l| l.number == line) {
                    self.scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
                    self.selected_row = Some(ix);
                    return;
                }
            }
        }
    }

    /// Lazily computed per-(file, side) syntax highlight, cached.
    pub fn highlight_for(&mut self, file_idx: usize, old_side: bool) -> bool {
        if self.highlights.contains_key(&(file_idx, old_side)) {
            return self.highlights[&(file_idx, old_side)].is_some();
        }
        let Some(changeset) = &self.changeset else {
            return false;
        };
        let file = &changeset.files[file_idx];
        let content = if old_side {
            &file.old_text
        } else {
            &file.new_text
        };
        let highlight = FileHighlight::new(content, &file.path);
        let present = highlight.is_some();
        self.highlights.insert((file_idx, old_side), highlight);
        present
    }
}
