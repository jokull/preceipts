// The content pane: find bar over the one scroll surface (multibuffer)
// over the statusbar. Sticky file header and find tick marks overlay the
// scroll. Chrome is semantic; the surface rows keep the One Dark palette.

import AppKit
import PreceiptsKit
import SwiftUI

protocol SurfaceDelegate: AnyObject {
    func surface(_ surface: SurfaceViewController, didScrollToFile fileIndex: Int)
    /// Drag-release / double-click / ⌘⇧M on a line range — open the tear
    /// in compose mode for this anchor.
    func surface(_ surface: SurfaceViewController, composeFor anchor: DraftAnchor)
    /// Double-click on a row with an anchored thread — open it.
    func surface(_ surface: SurfaceViewController, openThreadAtRow row: Int)
}

/// What a composed comment anchors to: contiguous line rows of one file,
/// end line + optional start line, new side preferred.
struct DraftAnchor {
    let path: String
    let line: Int
    let startLine: Int?
    let side: CommentSide
    let lineText: String
    /// Full selected block (this side's lines), for the clipboard.
    let quote: String?
    let rows: ClosedRange<Int>
}

final class SurfaceViewController: NSViewController {
    weak var delegate: SurfaceDelegate?

    private var changeset: Changeset?
    private var surface: Surface = .empty

    private let surfaceTable = NSTableView()
    private let scroll = NSScrollView()
    private let stickyHeader = DiffRowView()
    private let findBar = FindBarView()
    private let ticks = FindTicksView()
    /// HUD chips + file summary; the cockpit feeds hud/run state into it.
    let statusModel = StatusBarModel()
    /// Receipts panel (⌘J); the cockpit feeds status + run events into it.
    let receiptsPanel = ReceiptsPanelView()
    let prPanel = PrPanelView()

    private var findMatches: [Int] = []
    private var findCurrent = 0
    private var lastReportedFile = -1

    /// Case folding is paid once per changeset, off the main thread; the
    /// generation guard discards a stale index when reloads overlap.
    private var findIndex: FindIndex?
    private var findIndexGeneration = 0
    private var pendingFindQuery: String?

    /// Surface row → number of comments anchored there (drafts + GitHub)
    /// — the row bubbles; double-clicking one activates its thread in
    /// the thread area.
    private var commentBadges: [Int: Int] = [:]
    /// Live claw while drag-selecting a range — previews the would-be
    /// comment anchor; outranks the active thread's claw.
    private var previewClaw: (rows: ClosedRange<Int>, side: CommentSide)?
    private var dragSelecting = false

    /// The active thread's claw — the only conversation UI in the diff;
    /// reading happens in the thread area (trailing pane).
    private var threadAnchor: (rows: ClosedRange<Int>, side: CommentSide)?
    /// Esc with no find bar open — the cockpit pops the thread area.
    var onEscape: (() -> Void)?

    // ------------------------------------------------------------------
    // View construction

    override func loadView() {
        let column = NSTableColumn(identifier: .init("surface"))
        column.resizingMask = .autoresizingMask
        column.width = 800
        surfaceTable.addTableColumn(column)
        surfaceTable.headerView = nil
        surfaceTable.rowHeight = Theme.rowHeight
        surfaceTable.intercellSpacing = .zero
        surfaceTable.backgroundColor = Theme.bg
        surfaceTable.selectionHighlightStyle = .none
        surfaceTable.style = .plain
        surfaceTable.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        surfaceTable.delegate = self
        surfaceTable.dataSource = self
        surfaceTable.action = #selector(rowClicked(_:))
        surfaceTable.doubleAction = #selector(rowDoubleClicked(_:))
        surfaceTable.target = self
        surfaceTable.allowsMultipleSelection = true

        scroll.documentView = surfaceTable
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = true
        scroll.backgroundColor = Theme.bg

        // Container holding the scroll plus its overlays, top-left origin.
        let container = FlippedView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: container.topAnchor),
            scroll.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
        stickyHeader.isHidden = true
        ticks.isHidden = true
        container.clipsToBounds = true
        container.addSubview(stickyHeader)
        container.addSubview(ticks)

        let statusBar = makeStatusBar()

        findBar.isHidden = true
        findBar.onQueryChange = { [weak self] query in self?.updateFindMatches(query) }
        findBar.onStep = { [weak self] delta in self?.stepFindMatch(delta) }
        findBar.onClose = { [weak self] in self?.closeFind() }

        receiptsPanel.isHidden = true
        prPanel.isHidden = true

        let stack = NSStackView(views: [findBar, container, prPanel, receiptsPanel, statusBar])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.distribution = .fill
        container.setContentHuggingPriority(.init(1), for: .vertical)

        self.view = stack

        NSLayoutConstraint.activate([
            findBar.heightAnchor.constraint(equalToConstant: Metrics.findBarHeight),
            statusBar.heightAnchor.constraint(equalToConstant: Metrics.statusBarHeight),
            receiptsPanel.heightAnchor.constraint(equalToConstant: Metrics.receiptsPanelHeight),
            prPanel.heightAnchor.constraint(equalToConstant: Metrics.prPanelHeight),
            container.widthAnchor.constraint(equalTo: stack.widthAnchor),
            findBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
            receiptsPanel.widthAnchor.constraint(equalTo: stack.widthAnchor),
            prPanel.widthAnchor.constraint(equalTo: stack.widthAnchor),
            statusBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])

        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(
            self, selector: #selector(scrolled),
            name: NSView.boundsDidChangeNotification, object: scroll.contentView)
    }

    private func makeStatusBar() -> NSView {
        let bar = NSVisualEffectView()
        bar.material = .titlebar
        bar.blendingMode = .withinWindow

        let separator = NSBox()
        separator.boxType = .separator

        let chips = NSHostingView(rootView: StatusBarChips(model: statusModel))

        for view in [separator, chips] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            bar.addSubview(view)
        }
        NSLayoutConstraint.activate([
            separator.topAnchor.constraint(equalTo: bar.topAnchor),
            separator.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
            chips.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
            chips.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
            chips.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
        ])
        return bar
    }

    func toggleReceiptsPanel() {
        receiptsPanel.isHidden.toggle()
        if !receiptsPanel.isHidden {
            prPanel.isHidden = true
        }
    }

    func showReceiptsPanel() {
        receiptsPanel.isHidden = false
        prPanel.isHidden = true
    }

    /// One drawer at a time — PR and receipts share the bottom edge.
    func togglePrPanel() {
        prPanel.isHidden.toggle()
        if !prPanel.isHidden {
            receiptsPanel.isHidden = true
        }
    }

    var prPanelVisible: Bool { !prPanel.isHidden }

    // ------------------------------------------------------------------
    // Content

    func show(changeset: Changeset, surface: Surface) {
        // The claw indexes into the old surface — drop it before reload.
        clearClaw()
        self.changeset = changeset
        self.surface = surface
        lastReportedFile = -1
        surfaceTable.reloadData()
        updateStatusBar(changeset)
        rebuildFindIndex(changeset: changeset, surface: surface)
        updateOverlays()
    }

    private func rebuildFindIndex(changeset: Changeset, surface: Surface) {
        findIndex = nil
        findIndexGeneration += 1
        let generation = findIndexGeneration
        if findBar.isOpen, !findBar.query.isEmpty {
            pendingFindQuery = findBar.query
        }
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let index = FindIndex(surface: surface, changeset: changeset)
            DispatchQueue.main.async {
                guard let self, self.findIndexGeneration == generation else { return }
                self.findIndex = index
                if let pending = self.pendingFindQuery {
                    self.pendingFindQuery = nil
                    self.updateFindMatches(pending, keepPosition: true)
                }
            }
        }
    }

    func showError(_ message: String) {
        statusModel.message = message
    }

    private func updateStatusBar(_ changeset: Changeset) {
        statusModel.message = nil
        statusModel.filesCount = changeset.files.count
        statusModel.added = changeset.totalAdded
        statusModel.removed = changeset.totalRemoved
        switch changeset.scope {
        case .branch: statusModel.scopeLabel = "vs \(changeset.baseName)"
        case .uncommitted: statusModel.scopeLabel = "uncommitted"
        }
    }

    // ------------------------------------------------------------------
    // Scrolling + overlays

    private var rowHeight: CGFloat { Theme.rowHeight }
    private var scrollY: CGFloat { scroll.contentView.bounds.origin.y }
    private var topRow: Int { max(0, Int(scrollY / rowHeight)) }

    @objc private func scrolled() {
        updateOverlays()
    }

    private func updateOverlays() {
        guard !surface.rows.isEmpty, let changeset else {
            stickyHeader.isHidden = true
            ticks.isHidden = true
            return
        }
        // Sticky header: the current file's header pinned to the top,
        // pushed up by the next file's incoming header.
        if scrollY >= 0, let fileIndex = surface.fileIndex(atRow: topRow) {
            var pushUp: CGFloat = 0
            if fileIndex + 1 < surface.fileAnchors.count {
                let nextY = CGFloat(surface.fileAnchors[fileIndex + 1]) * rowHeight
                pushUp = min(0, nextY - scrollY - rowHeight)
            }
            stickyHeader.surfaceRow = .fileHeader(file: fileIndex)
            stickyHeader.changeset = changeset
            stickyHeader.frame = NSRect(
                x: 0, y: pushUp, width: scroll.frame.width, height: rowHeight)
            stickyHeader.isHidden = false
            stickyHeader.needsDisplay = true

            if fileIndex != lastReportedFile {
                lastReportedFile = fileIndex
                delegate?.surface(self, didScrollToFile: fileIndex)
            }
        } else {
            stickyHeader.isHidden = true
        }
        layoutTicks()
    }

    private func layoutTicks() {
        guard !findMatches.isEmpty else {
            ticks.isHidden = true
            return
        }
        ticks.frame = NSRect(
            x: scroll.frame.width - 10, y: 0, width: 10, height: scroll.frame.height)
        ticks.update(
            matches: findMatches, current: findCurrent, totalRows: surface.rows.count)
        ticks.isHidden = false
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        updateOverlays()
    }

    /// Scroll so `row` sits at the top (below the sticky header) or centered.
    func scrollToRow(_ row: Int, centered: Bool = false) {
        var y = CGFloat(row) * rowHeight
        if centered {
            y -= (scroll.contentView.bounds.height - rowHeight) / 2
        }
        let maxY = max(0, surfaceTable.frame.height - scroll.contentView.bounds.height)
        y = min(max(0, y), maxY)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    func scrollToFile(_ fileIndex: Int) {
        guard fileIndex < surface.fileAnchors.count else { return }
        lastReportedFile = fileIndex  // don't echo back to the sidebar
        scrollToRow(surface.fileAnchors[fileIndex])
    }

    func stepFile(_ delta: Int) {
        stepAnchor(surface.fileAnchors, delta)
    }

    func stepHunk(_ delta: Int) {
        stepAnchor(surface.hunkAnchors, delta)
    }

    private func stepAnchor(_ anchors: [Int], _ delta: Int) {
        guard !anchors.isEmpty else { return }
        let current = topRow
        let target: Int?
        if delta > 0 {
            target = anchors.first { $0 > current }
        } else {
            target = anchors.last { $0 < current }
        }
        if let target {
            scrollToRow(target)
        }
    }

    // ------------------------------------------------------------------
    // The claw (the active thread's range marker)

    func showClaw(rows: ClosedRange<Int>, side: CommentSide) {
        previewClaw = nil
        threadAnchor = (rows, side)
        scrollToRow(rows.lowerBound, centered: true)
        refreshVisibleRows()
    }

    func clearClaw() {
        guard threadAnchor != nil || previewClaw != nil else { return }
        threadAnchor = nil
        previewClaw = nil
        refreshVisibleRows()
    }

    override func cancelOperation(_ sender: Any?) {
        if findBar.isOpen {
            closeFind()
        } else {
            onEscape?()
        }
    }

    // ------------------------------------------------------------------
    // Comments (drafts + badges)

    func setCommentBadges(_ badges: [Int: Int]) {
        guard badges != commentBadges else { return }
        commentBadges = badges
        refreshVisibleRows()
    }

    private func selectionAnchor() -> DraftAnchor? {
        guard let changeset else { return nil }
        // Line rows only, all in the anchor file.
        let selected = selectedLineRows()
        guard let lowRow = selected.min(), let highRow = selected.max(),
            case .line(let file, _, _) = surface.rows[highRow]
        else { return nil }
        let fileDiff = changeset.files[file]

        func lineRef(_ row: Int, side: CommentSide) -> LineRef? {
            guard case .line(let f, let hunk, let index) = surface.rows[row], f == file
            else { return nil }
            let diffRow = fileDiff.hunks[hunk].rows[index]
            return side == .new ? diffRow.new : diffRow.old
        }

        // Side from the end row; the claw and GitHub both key on it.
        let side: CommentSide = lineRef(highRow, side: .new) != nil ? .new : .old
        guard let end = lineRef(highRow, side: side) else { return nil }
        // Walk the range's rows for the first one with a number this side,
        // gathering the block's text for the clipboard quote.
        var start: LineRef?
        var startRow = lowRow
        var blockLines: [String] = []
        for row in lowRow...highRow {
            if let ref = lineRef(row, side: side) {
                if start == nil {
                    start = ref
                    startRow = row
                }
                blockLines.append(ref.text)
            }
        }
        guard let start else { return nil }
        return DraftAnchor(
            path: fileDiff.path,
            line: end.number,
            startLine: start.number == end.number ? nil : start.number,
            side: side,
            lineText: end.text,
            quote: blockLines.count > 1 ? blockLines.joined(separator: "\n") : nil,
            rows: startRow...highRow)
    }

    /// ⌘⇧M / Add Comment / drag-release — tear opens in compose mode.
    func composeComment() {
        guard let anchor = selectionAnchor() else {
            NSSound.beep()
            return
        }
        delegate?.surface(self, composeFor: anchor)
    }

    func selectRow(_ row: Int) {
        guard row >= 0, row < surface.rows.count else { return }
        surfaceTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
    }

    /// Single click on a row's bubble opens its thread; clicks elsewhere
    /// keep their selection semantics.
    @objc private func rowClicked(_ sender: Any?) {
        let row = surfaceTable.clickedRow
        guard row >= 0, commentBadges[row] != nil, let event = NSApp.currentEvent
        else { return }
        let x = surfaceTable.convert(event.locationInWindow, from: nil).x
        if x > surfaceTable.bounds.width - 56 {
            delegate?.surface(self, openThreadAtRow: row)
        }
    }

    @objc private func rowDoubleClicked(_ sender: Any?) {
        let row = surfaceTable.clickedRow
        guard row >= 0 else { return }
        if commentBadges[row] != nil {
            delegate?.surface(self, openThreadAtRow: row)
        } else if case .line = surface.rows[row] {
            // Double-click a bare line: start a draft right there.
            selectRow(row)
            composeComment()
        }
    }

    // ------------------------------------------------------------------
    // Find

    func openFind() {
        findBar.isHidden = false
        findBar.focus()
    }

    func closeFind() {
        findBar.isHidden = true
        findBar.clear()
        findMatches = []
        findCurrent = 0
        refreshVisibleRows()
        layoutTicks()
        view.window?.makeFirstResponder(surfaceTable)
    }

    var isFindOpen: Bool { findBar.isOpen }

    func findNext() { stepFindMatch(1) }
    func findPrevious() { stepFindMatch(-1) }

    private func updateFindMatches(_ query: String, keepPosition: Bool = false) {
        guard let findIndex else {
            // Index still building (fresh reload) — run when it lands.
            pendingFindQuery = query
            return
        }
        let previous = keepPosition && findCurrent < findMatches.count
            ? findMatches[findCurrent] : nil
        findMatches = findIndex.matches(query: query)
        findCurrent = 0
        if let previous, let nearest = findMatches.firstIndex(where: { $0 >= previous }) {
            findCurrent = nearest
        }
        findBar.showCount(current: findCurrent, total: findMatches.count, query: query)
        if !keepPosition, findCurrent < findMatches.count {
            scrollToRow(findMatches[findCurrent], centered: true)
        }
        refreshVisibleRows()
        layoutTicks()
    }

    private func stepFindMatch(_ delta: Int) {
        guard !findMatches.isEmpty else { return }
        let count = findMatches.count
        findCurrent = ((findCurrent + delta) % count + count) % count
        findBar.showCount(current: findCurrent, total: count, query: findBar.query)
        scrollToRow(findMatches[findCurrent], centered: true)
        refreshVisibleRows()
        layoutTicks()
    }

    /// Repaint materialized rows with the current row state — cheaper than
    /// reloadData and keeps scroll position untouched.
    private func refreshVisibleRows() {
        surfaceTable.enumerateAvailableRowViews { [weak self] rowView, row in
            guard let self, let cell = rowView.view(atColumn: 0) as? DiffRowView
            else { return }
            self.configureRowState(cell, row: row)
            cell.needsDisplay = true
        }
        stickyHeader.needsDisplay = true
    }

    private func configureRowState(_ cell: DiffRowView, row: Int) {
        let query = findBar.isOpen ? findBar.query : ""
        cell.findQuery = query.isEmpty ? nil : query
        cell.isCurrentFindMatch =
            !findMatches.isEmpty && findCurrent < findMatches.count
            && findMatches[findCurrent] == row
        cell.isRowSelected = surfaceTable.selectedRowIndexes.contains(row)
        cell.commentBadge = commentBadges[row] ?? 0
        // Live drag/compose preview wins over the open thread's claw.
        let span = previewClaw ?? threadAnchor
        if let span, span.rows.contains(row) {
            let segment: DiffRowView.ClawSegment
            switch (row == span.rows.lowerBound, row == span.rows.upperBound) {
            case (true, true): segment = .single
            case (true, false): segment = .top
            case (false, true): segment = .bottom
            case (false, false): segment = .middle
            }
            cell.claw = (segment, span.side)
        } else {
            cell.claw = nil
        }
    }
}

// ------------------------------------------------------------------
// Table

extension SurfaceViewController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int {
        surface.rows.count
    }

    func tableView(
        _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
    ) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("diff-row")
        let cell =
            surfaceTable.makeView(withIdentifier: identifier, owner: nil) as? DiffRowView
            ?? {
                let view = DiffRowView()
                view.identifier = identifier
                return view
            }()
        cell.surfaceRow = surface.rows[row]
        cell.changeset = changeset
        configureRowState(cell, row: row)
        cell.needsDisplay = true
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        // Line rows select (comment anchoring); headers/gaps don't.
        if case .line = surface.rows[row] {
            return true
        }
        return false
    }

    /// Table-row selection filtered down to surface line rows.
    private func selectedLineRows() -> [Int] {
        surfaceTable.selectedRowIndexes.compactMap { index in
            if case .line = surface.rows[index] {
                return index
            }
            return nil
        }
    }

    /// Fires continuously while the mouse drags across rows — the claw
    /// previews the would-be comment range as it grows.
    func tableViewSelectionIsChanging(_ notification: Notification) {
        dragSelecting = true
        if let anchor = selectionAnchor() {
            previewClaw = (anchor.rows, anchor.side)
        } else {
            previewClaw = nil
        }
        refreshVisibleRows()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        // Releasing a multi-row drag goes straight into the composer.
        if dragSelecting {
            dragSelecting = false
            if selectedLineRows().count > 1 {
                composeComment()
            } else {
                previewClaw = nil
            }
        }
        refreshVisibleRows()
    }
}

/// Top-left-origin container so overlay math reads like the scroll offsets.
private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}
