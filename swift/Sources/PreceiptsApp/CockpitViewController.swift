// The cockpit brain: owns repo state and the single-flight coalesced
// reload discipline (the TUI storm lesson), splits into the file-tree
// sidebar and the diff surface, and carries the window toolbar (scope
// control, reload). Standard chrome primitives → Tahoe treatment for free.

import AppKit
import PreceiptsKit

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

    deinit {
        hudTimer?.invalidate()
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

        sidebar.delegate = self
        content.delegate = self

        requestReload()
    }

    // ------------------------------------------------------------------
    // Toolbar

    private enum ToolbarID {
        static let scope = NSToolbarItem.Identifier("preceipts.scope")
        static let reload = NSToolbarItem.Identifier("preceipts.reload")
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
        case .failure(let error):
            content.showError(error.localizedDescription)
        }
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
}

// ------------------------------------------------------------------
// Toolbar delegate

extension CockpitViewController: NSToolbarDelegate {
    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [
            .toggleSidebar, .sidebarTrackingSeparator, ToolbarID.scope, .flexibleSpace,
            ToolbarID.reload,
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
