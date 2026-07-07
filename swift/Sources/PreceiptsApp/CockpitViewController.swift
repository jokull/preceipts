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
    private let feedbackPanel = FeedbackPanelViewController()
    private var feedbackItem: NSSplitViewItem?

    /// Signed-in transport; nil falls back to the gh CLI.
    private var github: GitHubClient?
    private let prChipModel = PrChipModel()
    private var prTimer: Timer?
    private var prStatusInFlight = false

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
        let feedback = NSSplitViewItem(inspectorWithViewController: feedbackPanel)
        feedback.minimumThickness = 320
        feedback.maximumThickness = 560
        feedback.canCollapse = true
        feedback.isCollapsed = true
        addSplitViewItem(feedback)
        feedbackItem = feedback

        sidebar.delegate = self
        content.delegate = self
        feedbackPanel.delegate = self
        content.onThreadDismissed = { [weak self] in
            self?.feedbackPanel.clearSelection()
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
            // Anchor rows shifted with the new changeset; a mispointed
            // claw is worse than a closed card.
            content.dismissThread(notify: true)
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

    /// Panel list + surface badges from the current feedback and drafts —
    /// anchors re-resolve against every new changeset. Badges sit on each
    /// thread's root row and count the whole conversation.
    private func refreshFeedbackViews() {
        feedbackPanel.apply(feedback: prFeedback, drafts: commentStore?.comments ?? [])
        var badges: [Int: Int] = [:]
        guard let changeset else {
            content.setCommentBadges([:])
            return
        }
        for draft in commentStore?.comments ?? [] {
            if let row = surface.anchorRow(
                path: draft.path, line: draft.line, side: draft.side, changeset: changeset)
            {
                badges[row, default: 0] += 1
            }
        }
        for thread in FeedbackThread.group(prFeedback?.comments ?? []) {
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
        feedbackPanel.showStatus("Fetching PR feedback\u{2026}")
        // Signed in → URLSession; signed out → gh CLI; neither → explain.
        if let github, let repo = changeset?.githubRepo, let branch = changeset?.branch {
            Task { @MainActor [weak self] in
                do {
                    guard let feedback = try await github.fetchFeedback(
                        repo: repo, branch: branch)
                    else {
                        self?.feedbackPanel.showStatus(GhError.noPr.localizedDescription)
                        return
                    }
                    self?.prFeedback = feedback
                    self?.refreshFeedbackViews()
                } catch {
                    self?.feedbackPanel.showStatus(error.localizedDescription)
                }
            }
            return
        }
        guard let gh, gh.available else {
            feedbackPanel.showStatus(
                "Sign in to GitHub in Settings (\u{2318},) \u{2014} or install the gh CLI")
            return
        }
        gh.fetchFeedback { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let feedback):
                self.prFeedback = feedback
                self.refreshFeedbackViews()
            case .failure(let error):
                self.feedbackPanel.showStatus(error.localizedDescription)
            }
        }
    }

    @objc func toggleFeedbackPanel(_ sender: Any?) {
        guard let feedbackItem else { return }
        feedbackItem.animator().isCollapsed.toggle()
        if !feedbackItem.isCollapsed, prFeedback == nil {
            refreshFeedback()
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

    func surface(_ surface: SurfaceViewController, didScrollToFile fileIndex: Int) {
        sidebar.highlight(fileIndex: fileIndex)
    }

    func surface(
        _ surface: SurfaceViewController,
        addDraft path: String, line: Int, startLine: Int?, side: CommentSide,
        lineText: String, body: String
    ) {
        try? commentStore?.add(
            path: path, line: line, startLine: startLine, side: side,
            lineText: lineText, body: body)
        refreshFeedbackViews()
    }

    /// Double-click on a badged row: open the thread rooted there.
    func surface(_ surface: SurfaceViewController, openThreadAtRow row: Int) {
        guard let item = navItem(atRow: row) else { return }
        feedbackPanel.highlight(item)
        content.presentThread(
            threadCardModel(item),
            anchorRows: resolveAnchorRows(item),
            side: item.anchor?.side ?? .new)
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

extension CockpitViewController: FeedbackPanelDelegate {
    func feedbackPanelRequestsRefresh(_ panel: FeedbackPanelViewController) {
        refreshFeedback()
    }

    func feedbackPanel(
        _ panel: FeedbackPanelViewController, didSelect item: FeedbackNavItem?
    ) {
        guard let item else {
            content.dismissThread()
            return
        }
        content.presentThread(
            threadCardModel(item),
            anchorRows: resolveAnchorRows(item),
            side: item.anchor?.side ?? .new)
    }

    func feedbackPanel(_ panel: FeedbackPanelViewController, deleteDraft id: UInt64) {
        try? commentStore?.remove(id: id)
        content.dismissThread()
        refreshFeedbackViews()
    }

    func feedbackPanel(
        _ panel: FeedbackPanelViewController, anchorRowFor item: FeedbackNavItem
    ) -> Int? {
        resolveAnchorRows(item)?.lowerBound
    }

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

    private func threadCardModel(_ item: FeedbackNavItem) -> ThreadCardModel {
        switch item {
        case .thread(let thread):
            let title: String
            if let path = thread.root.path, let range = thread.lineRange {
                title =
                    range.count == 1
                    ? "\(path):\(range.lowerBound)"
                    : "\(path):\(range.lowerBound)\u{2013}\(range.upperBound)"
            } else if let state = thread.root.state {
                title = state.lowercased().replacingOccurrences(of: "_", with: " ")
            } else {
                title = "Conversation"
            }
            return ThreadCardModel(
                title: title,
                entries: thread.comments.map { comment in
                    ThreadCardModel.Entry(
                        author: comment.author,
                        meta: Self.relativeTime(comment.createdAt),
                        body: comment.body)
                },
                url: thread.root.url,
                digest: formatDigest(thread.comments.map(\.context)))
        case .draft(let draft):
            return ThreadCardModel(
                title: "\(draft.path):\(draft.line)",
                entries: [
                    ThreadCardModel.Entry(
                        author: "Draft", meta: "never posted", body: draft.body)
                ],
                url: nil,
                digest: item.contexts.map(formatComment).joined(separator: "\n"))
        }
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
