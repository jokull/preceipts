// The cockpit brain: owns repo state and the single-flight coalesced
// reload discipline (the TUI storm lesson), splits into the file-tree
// sidebar and the diff surface, and carries the window toolbar (scope
// control, reload). Standard chrome primitives → Tahoe treatment for free.

import AppKit
import PreceiptsKit
import SwiftUI

final class CockpitViewController: NSSplitViewController {
    private let repo: URL
    private var scope: DiffScope = .branch
    private var changeset: Changeset?
    private var surface: Surface = .empty

    private var reloadInFlight = false
    private var reloadQueued = false
    private var watcher: Watcher?

    private var engine: EngineClient?
    private var hudTimer: Timer?

    private var gh: GhClient?
    private var commentStore: CommentStore?
    private var commentStoreBranch: String?
    private var prFeedback: PrFeedback?
    private let threadArea = ThreadAreaViewController()
    private var feedbackItem: NSSplitViewItem?

    /// Signed-in transport; nil falls back to the gh CLI.
    private var github: GitHubClient?
    private let prChipModel = PrChipModel()
    private var prTimer: Timer?
    private var prStatusInFlight = false

    /// The thread area's navigation stack: conversation is the empty
    /// state, threads/compose sit one level in, back pops.
    private enum TearState {
        case closed
        case conversation
        case thread(FeedbackNavItem)
        case compose(DraftAnchor)
    }
    private var tearState: TearState = .closed

    deinit {
        hudTimer?.invalidate()
        prTimer?.invalidate()
    }

    private let sidebar = SidebarViewController()
    private let content = SurfaceViewController()
    private var scopeControl: NSSegmentedControl?

    init(repo: URL) {
        self.repo = repo
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidLoad() {
        super.viewDidLoad()
        splitView.autosaveName = "PreceiptsCockpitSplit"

        let sidebarItem = NSSplitViewItem(sidebarWithViewController: sidebar)
        sidebarItem.minimumThickness = Metrics.sidebarMinWidth
        sidebarItem.preferredThicknessFraction = 0.2
        addSplitViewItem(sidebarItem)

        let contentItem = NSSplitViewItem(viewController: content)
        contentItem.minimumThickness = Metrics.surfaceMinWidth
        addSplitViewItem(contentItem)

        // Feedback as an inspector pane: trailing material, collapsible.
        let feedback = NSSplitViewItem(inspectorWithViewController: threadArea)
        feedback.minimumThickness = 320
        feedback.maximumThickness = 560
        feedback.canCollapse = true
        feedback.isCollapsed = true
        addSplitViewItem(feedback)
        feedbackItem = feedback

        sidebar.delegate = self
        content.delegate = self
        threadArea.onRefresh = { [weak self] in
            self?.refreshFeedback()
        }
        // Esc pops the thread area one level: thread → conversation →
        // pane collapsed.
        content.onEscape = { [weak self] in
            guard let self else { return }
            switch self.tearState {
            case .thread, .compose:
                self.setTear(.conversation)
            case .conversation:
                self.setTear(.closed)
            case .closed:
                break
            }
        }

        if let token = Keychain.loadToken() {
            github = GitHubClient(token: token)
        }
        NotificationCenter.default.addObserver(
            self, selector: #selector(authChanged),
            name: .gitHubAuthChanged, object: nil)
        NotificationCenter.default.addObserver(
            self, selector: #selector(windowFocused),
            name: NSWindow.didBecomeKeyNotification, object: nil)
        prTimer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            self?.refreshPrStatus()
        }

        requestReload()
    }

    @objc private func authChanged() {
        github = Keychain.loadToken().map(GitHubClient.init(token:))
        prFeedback = nil
        refreshPrStatus()
        if feedbackItem?.isCollapsed == false {
            refreshFeedback()
        }
    }

    @objc private func windowFocused(_ notification: Notification) {
        guard (notification.object as? NSWindow) === view.window else { return }
        refreshPrStatus()
    }

    /// Poll politely: timer + window focus + after reloads; single-flight.
    private func refreshPrStatus() {
        guard let changeset, !prStatusInFlight else { return }
        prStatusInFlight = true
        if let github, let repo = changeset.githubRepo, let branch = changeset.branch {
            Task { @MainActor [weak self] in
                let status = try? await github.prStatus(repo: repo, branch: branch)
                self?.prChipModel.status = status
                self?.prStatusInFlight = false
            }
        } else if let gh, gh.available {
            gh.fetchPrStatus { [weak self] status in
                self?.prChipModel.status = status
                self?.prStatusInFlight = false
            }
        } else {
            prStatusInFlight = false
        }
    }

    // ------------------------------------------------------------------
    // Toolbar

    private enum ToolbarID {
        static let scope = NSToolbarItem.Identifier("preceipts.scope")
        static let reload = NSToolbarItem.Identifier("preceipts.reload")
        static let pr = NSToolbarItem.Identifier("preceipts.pr")
    }

    func makeToolbar() -> NSToolbar {
        let toolbar = NSToolbar(identifier: "preceipts.main")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = false
        return toolbar
    }

    // ------------------------------------------------------------------
    // Actions (responder chain targets for menus + toolbar)

    @objc func reload(_ sender: Any?) {
        requestReload()
    }

    @objc func toggleScope(_ sender: Any?) {
        setScope(scope == .branch ? .uncommitted : .branch)
    }

    @objc func selectBranchScope(_ sender: Any?) {
        setScope(.branch)
    }

    @objc func selectUncommittedScope(_ sender: Any?) {
        setScope(.uncommitted)
    }

    @objc private func scopeControlChanged(_ sender: NSSegmentedControl) {
        setScope(sender.selectedSegment == 0 ? .branch : .uncommitted)
    }

    private func setScope(_ newScope: DiffScope) {
        guard scope != newScope else { return }
        scope = newScope
        scopeControl?.selectedSegment = newScope == .branch ? 0 : 1
        requestReload()
    }

    @objc func openFind(_ sender: Any?) {
        content.openFind()
    }

    @objc func findNext(_ sender: Any?) {
        content.findNext()
    }

    @objc func findPrevious(_ sender: Any?) {
        content.findPrevious()
    }

    @objc func nextFile(_ sender: Any?) {
        content.stepFile(1)
    }

    @objc func previousFile(_ sender: Any?) {
        content.stepFile(-1)
    }

    @objc func nextHunk(_ sender: Any?) {
        content.stepHunk(1)
    }

    @objc func previousHunk(_ sender: Any?) {
        content.stepHunk(-1)
    }

    // ------------------------------------------------------------------
    // Loading (single-flight, coalesced)

    private func requestReload() {
        if reloadInFlight {
            reloadQueued = true
            return
        }
        reloadInFlight = true
        let repo = repo
        let scope = scope
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = Result { try ChangesetLoader.load(repoPath: repo, scope: scope) }
            DispatchQueue.main.async {
                guard let self else { return }
                self.reloadInFlight = false
                self.apply(result)
                if self.reloadQueued {
                    self.reloadQueued = false
                    self.requestReload()
                }
            }
        }
    }

    private func apply(_ result: Result<Changeset, Error>) {
        switch result {
        case .success(let changeset):
            self.changeset = changeset
            surface = Surface.build(changeset)
            ensureWatcher(changeset)
            content.show(changeset: changeset, surface: surface)
            sidebar.show(tree: FileTree.build(changeset.files))
            updateWindowTitle(changeset)
            ensureEngine(changeset)
            refreshHud()
            ensureCommentStore(changeset)
            // Anchor rows shifted with the new changeset — pop the tear
            // to the safe level rather than point the claw at the wrong
            // lines. (content.show already dropped the stale tear.)
            switch tearState {
            case .closed:
                break
            default:
                setTear(.conversation)
            }
            refreshFeedbackViews()
            refreshPrStatus()
        case .failure(let error):
            content.showError(error.localizedDescription)
        }
    }

    // ------------------------------------------------------------------
    // Feedback + drafts

    private func ensureCommentStore(_ changeset: Changeset) {
        if gh == nil {
            gh = GhClient(workdir: changeset.workdir)
        }
        let branch = changeset.branch ?? "(detached)"
        if commentStore == nil || commentStoreBranch != branch {
            commentStore = try? CommentStore(gitDir: changeset.gitDir, branch: branch)
            commentStoreBranch = branch
        }
    }

    /// Navigator list, sidebar bubbles, and the row→thread routing map,
    /// from the current feedback and drafts — anchors re-resolve against
    /// every new changeset.
    private func refreshFeedbackViews() {
        let drafts = commentStore?.comments ?? []

        // File-tree bubbles: unresolved threads + drafts per path.
        var counts: [String: Int] = [:]
        for draft in drafts {
            counts[draft.path, default: 0] += 1
        }
        let threads = FeedbackThread.group(prFeedback?.comments ?? [])
        for thread in threads {
            guard let path = thread.root.path,
                prFeedback?.resolvedRootIds.contains(thread.root.id) != true
            else { continue }
            counts[path, default: 0] += 1
        }
        sidebar.updateCommentCounts(counts)

        // Row routing for double-clicks (not drawn).
        var badges: [Int: Int] = [:]
        guard let changeset else {
            content.setCommentBadges([:])
            return
        }
        for draft in drafts {
            if let row = surface.anchorRow(
                path: draft.path, line: draft.line, side: draft.side, changeset: changeset)
            {
                badges[row, default: 0] += 1
            }
        }
        for thread in threads {
            if let path = thread.root.path, let line = thread.root.line,
                let row = surface.anchorRow(
                    path: path, line: line, side: thread.root.side, changeset: changeset)
            {
                badges[row, default: 0] += thread.comments.count
            }
        }
        content.setCommentBadges(badges)
    }

    private func refreshFeedback() {
        threadArea.showStatus("Fetching PR feedback\u{2026}")
        // Signed in → URLSession; signed out → gh CLI; neither → explain.
        if let github, let repo = changeset?.githubRepo, let branch = changeset?.branch {
            Task { @MainActor [weak self] in
                do {
                    guard let feedback = try await github.fetchFeedback(
                        repo: repo, branch: branch)
                    else {
                        self?.threadArea.showStatus(GhError.noPr.localizedDescription)
                        return
                    }
                    self?.prFeedback = feedback
                    self?.refreshFeedbackViews()
                } catch {
                    self?.threadArea.showStatus(error.localizedDescription)
                }
            }
            return
        }
        guard let gh, gh.available else {
            threadArea.showStatus(
                "Sign in to GitHub in Settings (\u{2318},) \u{2014} or install the gh CLI")
            return
        }
        gh.fetchFeedback(repo: changeset?.githubRepo) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let feedback):
                self.prFeedback = feedback
                self.refreshFeedbackViews()
            case .failure(let error):
                self.threadArea.showStatus(error.localizedDescription)
            }
        }
    }

    @objc func toggleFeedbackPanel(_ sender: Any?) {
        guard let feedbackItem else { return }
        feedbackItem.animator().isCollapsed.toggle()
        if !feedbackItem.isCollapsed {
            if prFeedback == nil {
                refreshFeedback()
            }
            if case .closed = tearState {
                setTear(.conversation)
            }
        }
    }

    @objc func addComment(_ sender: Any?) {
        content.composeComment()
    }

    // ------------------------------------------------------------------
    // Engine (hud + runs; timer + watch-event refresh, single-flight)

    private func ensureEngine(_ changeset: Changeset) {
        guard engine == nil else { return }
        let engine = EngineClient(workdir: changeset.workdir)
        self.engine = engine
        content.statusModel.onReceiptsTap = { [weak self] in
            self?.toggleReceiptsPanel(nil)
        }
        content.receiptsPanel.onRunClicked = { [weak self] in
            self?.runChecks(nil)
        }
        guard engine.available else {
            content.statusModel.hudError = "preceipts-engine not found"
            return
        }
        hudTimer = Timer.scheduledTimer(withTimeInterval: 45, repeats: true) { [weak self] _ in
            self?.refreshHud()
        }
    }

    private func refreshHud() {
        guard let engine, engine.available, let changeset else { return }
        engine.refreshHud(base: changeset.baseBranch) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let hud):
                self.content.statusModel.hud = hud
                self.content.statusModel.hudError = nil
                self.content.receiptsPanel.apply(status: hud.status)
            case .failure(let error):
                self.content.statusModel.hudError = error.localizedDescription
            }
        }
    }

    @objc func runChecks(_ sender: Any?) {
        guard let engine, engine.available, !engine.runInProgress else { return }
        content.showReceiptsPanel()
        content.statusModel.runInProgress = true
        content.receiptsPanel.beginRun()
        engine.runChecks(
            onEvent: { [weak self] event in
                self?.content.receiptsPanel.handle(event)
            },
            completion: { [weak self] outcome in
                guard let self else { return }
                self.content.statusModel.runInProgress = false
                self.content.receiptsPanel.endRun(outcome)
                self.refreshHud()
            })
    }

    @objc func toggleReceiptsPanel(_ sender: Any?) {
        content.toggleReceiptsPanel()
    }

    private func ensureWatcher(_ changeset: Changeset) {
        guard watcher == nil else { return }
        watcher = Watcher(workdir: changeset.workdir) { [weak self] in
            self?.requestReload()
        }
    }

    private func updateWindowTitle(_ changeset: Changeset) {
        guard let window = view.window else { return }
        window.title = repo.lastPathComponent
        window.subtitle = changeset.branch ?? "detached HEAD"
    }
}

// ------------------------------------------------------------------
// Sidebar ⇄ surface (two views of one model)

extension CockpitViewController: SidebarDelegate, SurfaceDelegate {
    func sidebar(_ sidebar: SidebarViewController, didSelectFile fileIndex: Int) {
        content.scrollToFile(fileIndex)
    }

    /// Sidebar bubble → the first unresolved thread under that path
    /// (files exactly, directories by prefix), drafts as fallback.
    func sidebar(_ sidebar: SidebarViewController, openThreadsForPath path: String) {
        let prefix = path + "/"
        func matches(_ candidate: String) -> Bool {
            candidate == path || candidate.hasPrefix(prefix)
        }
        let unresolved = FeedbackThread.group(prFeedback?.comments ?? [])
            .filter { thread in
                guard let threadPath = thread.root.path, matches(threadPath) else {
                    return false
                }
                return prFeedback?.resolvedRootIds.contains(thread.root.id) != true
            }
            .sorted {
                ($0.root.path ?? "", $0.root.line ?? 0) < ($1.root.path ?? "", $1.root.line ?? 0)
            }
        if let first = unresolved.first {
            setTear(.thread(.thread(first)))
        } else if let draft = (commentStore?.comments ?? []).first(where: { matches($0.path) }) {
            setTear(.thread(.draft(draft)))
        }
    }

    func surface(_ surface: SurfaceViewController, didScrollToFile fileIndex: Int) {
        sidebar.highlight(fileIndex: fileIndex)
    }

    func surface(_ surface: SurfaceViewController, composeFor anchor: DraftAnchor) {
        setTear(.compose(anchor))
    }

    /// Double-click on a row bubble: open its thread in the thread area.
    func surface(_ surface: SurfaceViewController, openThreadAtRow row: Int) {
        guard let item = navItem(atRow: row) else { return }
        setTear(.thread(item))
    }

    private func navItem(atRow row: Int) -> FeedbackNavItem? {
        for draft in commentStore?.comments ?? [] {
            let item = FeedbackNavItem.draft(draft)
            if let rows = resolveAnchorRows(item), rows.contains(row) {
                return item
            }
        }
        for thread in FeedbackThread.group(prFeedback?.comments ?? []) {
            let item = FeedbackNavItem.thread(thread)
            if let rows = resolveAnchorRows(item), rows.contains(row) {
                return item
            }
        }
        return nil
    }
}

extension CockpitViewController {
    /// Surface-row span for an item's line range; nil when it no longer
    /// resolves in this changeset (partial ranges clamp to what does).
    private func resolveAnchorRows(_ item: FeedbackNavItem) -> ClosedRange<Int>? {
        guard let changeset, let anchor = item.anchor else { return nil }
        var resolved: [Int] = []
        for line in anchor.range {
            if let row = surface.anchorRow(
                path: anchor.path, line: line, side: anchor.side, changeset: changeset)
            {
                resolved.append(row)
            }
        }
        guard let low = resolved.min(), let high = resolved.max() else { return nil }
        // Drafts also go outdated when the anchored (end) line's text
        // changed under them.
        if case .draft(let draft) = item,
            let endRow = surface.anchorRow(
                path: anchor.path, line: draft.line, side: draft.side, changeset: changeset),
            case .line(let file, let hunk, let rowIndex) = surface.rows[endRow]
        {
            let diffRow = changeset.files[file].hunks[hunk].rows[rowIndex]
            let current = draft.side == .new ? diffRow.new?.text : diffRow.old?.text
            if current != draft.lineText {
                return nil
            }
        }
        return low...high
    }

    // ------------------------------------------------------------------
    // The thread area's state machine

    private func setTear(_ state: TearState) {
        tearState = state
        switch state {
        case .closed:
            content.clearClaw()
            feedbackItem?.animator().isCollapsed = true
        case .conversation:
            revealThreadArea()
            content.clearClaw()
            threadArea.render(conversationContent(), handlers: conversationHandlers())
        case .thread(let item):
            revealThreadArea()
            threadArea.render(threadContent(item), handlers: threadHandlers(item))
            if let rows = resolveAnchorRows(item) {
                content.showClaw(rows: rows, side: item.anchor?.side ?? .new)
            } else {
                content.clearClaw()
            }
        case .compose(let anchor):
            revealThreadArea()
            threadArea.render(composeContent(anchor), handlers: composeHandlers(anchor))
            content.showClaw(rows: anchor.rows, side: anchor.side)
        }
    }

    private func revealThreadArea() {
        if feedbackItem?.isCollapsed == true {
            feedbackItem?.animator().isCollapsed = false
        }
    }

    /// Store path for PR-level draft notes (no file anchor).
    static let conversationPath = "(conversation)"

    private var conversationNotes: [LocalComment] {
        (commentStore?.comments ?? []).filter { $0.path == Self.conversationPath }
    }

    private func conversationContent() -> TearContent {
        let notes = conversationNotes
        guard let feedback = prFeedback else {
            return TearContent(
                breadcrumb: "Conversation", showBack: false, entries: [], notes: notes,
                url: nil, digest: formatDigest(notes.map(noteContext(_:))),
                composerPlaceholder: "Add a PR note", focusComposer: false,
                emptyText: notes.isEmpty
                    ? "No PR feedback loaded \u{2014} refresh (\u{21bb}) or add a note" : nil)
        }
        let prLevel = FeedbackThread.group(feedback.comments)
            .filter { $0.root.path == nil }
            .flatMap(\.comments)
            .sorted { $0.createdAt < $1.createdAt }
        return TearContent(
            breadcrumb: "#\(feedback.number)  \(feedback.title)",
            showBack: false,
            entries: prLevel.map(entry(_:)),
            notes: notes,
            url: feedback.url,
            digest: formatDigest(
                prLevel.map(\.context) + notes.map(noteContext(_:))),
            composerPlaceholder: "Add a PR note",
            focusComposer: false,
            emptyText: (prLevel.isEmpty && notes.isEmpty)
                ? "No PR-level conversation yet \u{2014} click a bubble in the diff"
                : nil)
    }

    private func noteContext(_ note: LocalComment) -> CommentContext {
        CommentContext(
            path: note.path, line: note.line, startLine: note.startLine,
            lineText: note.quote ?? note.lineText, body: note.body, author: nil)
    }

    private func conversationHandlers() -> TearHandlers {
        var handlers = TearHandlers()
        handlers.saveNote = { [weak self] body in
            guard let self else { return }
            try? self.commentStore?.add(
                path: Self.conversationPath, line: 0, side: .new, lineText: "", body: body)
            self.refreshFeedbackViews()
            self.setTear(.conversation)
        }
        handlers.updateNote = { [weak self] id, body in
            try? self?.commentStore?.updateBody(id: id, body: body)
            self?.refreshFeedbackViews()
        }
        handlers.deleteNote = { [weak self] id in
            try? self?.commentStore?.remove(id: id)
            self?.refreshFeedbackViews()
            self?.setTear(.conversation)
        }
        return handlers
    }

    private func threadContent(_ item: FeedbackNavItem) -> TearContent {
        let breadcrumb: String
        if let anchor = item.anchor {
            breadcrumb =
                anchor.range.count == 1
                ? "\(anchor.path):\(anchor.range.lowerBound)"
                : "\(anchor.path):\(anchor.range.lowerBound)\u{2013}\(anchor.range.upperBound)"
        } else {
            breadcrumb = "Conversation"
        }
        switch item {
        case .thread(let thread):
            return TearContent(
                breadcrumb: breadcrumb,
                showBack: true,
                entries: thread.comments.map(entry(_:)),
                notes: notesMatching(item),
                url: thread.root.url,
                digest: formatDigest(thread.comments.map(\.context)),
                composerPlaceholder: "Add a draft note",
                focusComposer: false,
                emptyText: nil)
        case .draft(let draft):
            return TearContent(
                breadcrumb: breadcrumb,
                showBack: true,
                entries: [],
                notes: [draft],
                url: nil,
                digest: item.contexts.map(formatComment).joined(separator: "\n"),
                composerPlaceholder: "Add a draft note",
                focusComposer: false,
                emptyText: nil)
        }
    }

    private func composeContent(_ anchor: DraftAnchor) -> TearContent {
        let range =
            anchor.startLine.map { "\(min($0, anchor.line))\u{2013}\(anchor.line)" }
            ?? "\(anchor.line)"
        return TearContent(
            breadcrumb: "\(anchor.path):\(range)",
            showBack: true,
            entries: [], notes: [],
            url: nil, digest: "",
            composerPlaceholder: "Draft comment",
            focusComposer: true,
            emptyText: nil)
    }

    /// Drafts sharing a GitHub thread's exact anchor render inside it as
    /// your notes on that conversation.
    private func notesMatching(_ item: FeedbackNavItem) -> [LocalComment] {
        guard case .thread = item, let anchor = item.anchor else { return [] }
        return (commentStore?.comments ?? []).filter {
            $0.path == anchor.path && $0.lineRange == anchor.range && $0.side == anchor.side
        }
    }

    private func threadHandlers(_ item: FeedbackNavItem) -> TearHandlers {
        var handlers = TearHandlers()
        handlers.back = { [weak self] in
            self?.setTear(.conversation)
        }
        handlers.updateNote = { [weak self] id, body in
            try? self?.commentStore?.updateBody(id: id, body: body)
            self?.refreshFeedbackViews()
        }
        handlers.deleteNote = { [weak self] id in
            guard let self else { return }
            try? self.commentStore?.remove(id: id)
            self.refreshFeedbackViews()
            if case .draft(let draft) = item, draft.id == id {
                self.setTear(.conversation)
            } else {
                self.setTear(.thread(item))
            }
        }
        if let anchor = item.anchor {
            let lineText: String
            switch item {
            case .thread(let thread): lineText = thread.root.lineText
            case .draft(let draft): lineText = draft.lineText
            }
            handlers.saveNote = { [weak self] body in
                guard let self else { return }
                try? self.commentStore?.add(
                    path: anchor.path,
                    line: anchor.range.upperBound,
                    startLine: anchor.range.count > 1 ? anchor.range.lowerBound : nil,
                    side: anchor.side,
                    lineText: lineText,
                    body: body)
                self.refreshFeedbackViews()
                self.setTear(.thread(item))
            }
        }
        return handlers
    }

    private func composeHandlers(_ anchor: DraftAnchor) -> TearHandlers {
        var handlers = TearHandlers()
        handlers.back = { [weak self] in
            self?.setTear(.conversation)
        }
        handlers.saveNote = { [weak self] body in
            guard let self,
                let draft = try? self.commentStore?.add(
                    path: anchor.path, line: anchor.line, startLine: anchor.startLine,
                    side: anchor.side, lineText: anchor.lineText, quote: anchor.quote,
                    body: body)
            else { return }
            self.refreshFeedbackViews()
            self.setTear(.thread(.draft(draft)))
        }
        return handlers
    }

    private func entry(_ comment: FeedbackComment) -> TearContent.Entry {
        var author = comment.author
        if let state = comment.state, !state.isEmpty {
            author += " \u{00b7} " + state.lowercased().replacingOccurrences(of: "_", with: " ")
        }
        return TearContent.Entry(
            author: author,
            meta: Self.relativeTime(comment.createdAt),
            body: comment.body)
    }

    private static func relativeTime(_ iso: String) -> String {
        guard let date = ISO8601DateFormatter().date(from: iso) else { return "" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }
}

// ------------------------------------------------------------------
// Toolbar delegate

extension CockpitViewController: NSToolbarDelegate {
    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [
            .toggleSidebar, .sidebarTrackingSeparator, ToolbarID.scope, .flexibleSpace,
            ToolbarID.pr, ToolbarID.reload,
        ]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }

    func toolbar(
        _ toolbar: NSToolbar,
        itemForItemIdentifier itemIdentifier: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar flag: Bool
    ) -> NSToolbarItem? {
        switch itemIdentifier {
        case ToolbarID.scope:
            let control = NSSegmentedControl(
                labels: ["Branch diff", "Uncommitted"],
                trackingMode: .selectOne,
                target: self,
                action: #selector(scopeControlChanged(_:)))
            control.selectedSegment = scope == .branch ? 0 : 1
            scopeControl = control
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.view = control
            item.label = "Scope"
            item.toolTip = "Diff scope (\u{2318}\u{21e7}D)"
            return item
        case ToolbarID.pr:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            let host = NSHostingView(rootView: PrChipView(model: prChipModel))
            host.sizingOptions = .intrinsicContentSize
            item.view = host
            item.label = "Pull Request"
            return item
        case ToolbarID.reload:
            let item = NSToolbarItem(itemIdentifier: itemIdentifier)
            item.image = NSImage(
                systemSymbolName: "arrow.clockwise", accessibilityDescription: "reload")
            item.label = "Reload"
            item.toolTip = "Reload the diff (\u{2318}R)"
            item.isBordered = true
            item.target = self
            item.action = #selector(reload(_:))
            return item
        default:
            return nil
        }
    }
}
