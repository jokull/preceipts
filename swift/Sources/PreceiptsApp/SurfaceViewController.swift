// The content pane: find bar over the one scroll surface (multibuffer)
// over the statusbar. Sticky file header and find tick marks overlay the
// scroll. Chrome is semantic; the surface rows keep the One Dark palette.

import AppKit
import PreceiptsKit

protocol SurfaceDelegate: AnyObject {
    func surface(_ surface: SurfaceViewController, didScrollToFile fileIndex: Int)
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
    private let statusLeft = NSTextField(labelWithString: "Loading\u{2026}")
    private let statusRight = NSTextField(labelWithString: "")

    private var findMatches: [Int] = []
    private var findCurrent = 0
    private var lastReportedFile = -1

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
        container.addSubview(stickyHeader)
        container.addSubview(ticks)

        let statusBar = makeStatusBar()

        findBar.isHidden = true
        findBar.onQueryChange = { [weak self] query in self?.updateFindMatches(query) }
        findBar.onStep = { [weak self] delta in self?.stepFindMatch(delta) }
        findBar.onClose = { [weak self] in self?.closeFind() }

        let stack = NSStackView(views: [findBar, container, statusBar])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.distribution = .fill
        container.setContentHuggingPriority(.init(1), for: .vertical)

        self.view = stack

        NSLayoutConstraint.activate([
            findBar.heightAnchor.constraint(equalToConstant: Metrics.findBarHeight),
            statusBar.heightAnchor.constraint(equalToConstant: Metrics.statusBarHeight),
            container.widthAnchor.constraint(equalTo: stack.widthAnchor),
            findBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
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

        statusLeft.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        statusLeft.textColor = .labelColor
        statusRight.font = .monospacedDigitSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .regular)
        statusRight.textColor = .secondaryLabelColor
        for label in [statusLeft, statusRight] {
            label.lineBreakMode = .byTruncatingTail
        }

        for view in [separator, statusLeft, statusRight] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            bar.addSubview(view)
        }
        NSLayoutConstraint.activate([
            separator.topAnchor.constraint(equalTo: bar.topAnchor),
            separator.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
            statusLeft.leadingAnchor.constraint(
                equalTo: bar.leadingAnchor, constant: Metrics.paddingWide),
            statusLeft.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
            statusRight.trailingAnchor.constraint(
                equalTo: bar.trailingAnchor, constant: -Metrics.paddingWide),
            statusRight.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
            statusLeft.trailingAnchor.constraint(
                lessThanOrEqualTo: statusRight.leadingAnchor, constant: -Metrics.paddingWide),
        ])
        return bar
    }

    // ------------------------------------------------------------------
    // Content

    func show(changeset: Changeset, surface: Surface) {
        self.changeset = changeset
        self.surface = surface
        lastReportedFile = -1
        surfaceTable.reloadData()
        updateStatusBar(changeset)
        if findBar.isOpen {
            updateFindMatches(findBar.query, keepPosition: true)
        }
        updateOverlays()
    }

    func showError(_ message: String) {
        statusLeft.stringValue = message
    }

    private func updateStatusBar(_ changeset: Changeset) {
        let scopeLabel: String
        switch changeset.scope {
        case .branch: scopeLabel = "Branch diff vs \(changeset.baseName)"
        case .uncommitted: scopeLabel = "Uncommitted changes"
        }
        statusLeft.stringValue = "\(scopeLabel) \u{2014} \(changeset.branch ?? "detached HEAD")"
        statusRight.stringValue =
            "\(changeset.files.count) files   +\(changeset.totalAdded) \u{2212}\(changeset.totalRemoved)"
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
        guard let changeset else { return }
        let previous = keepPosition && findCurrent < findMatches.count
            ? findMatches[findCurrent] : nil
        findMatches = FindMatcher.matches(query: query, surface: surface, changeset: changeset)
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

    /// Repaint materialized rows with the current find state — cheaper than
    /// reloadData and keeps scroll position untouched.
    private func refreshVisibleRows() {
        surfaceTable.enumerateAvailableRowViews { [weak self] rowView, row in
            guard let self, let cell = rowView.view(atColumn: 0) as? DiffRowView else { return }
            self.configureFindState(cell, row: row)
            cell.needsDisplay = true
        }
        stickyHeader.needsDisplay = true
    }

    private func configureFindState(_ cell: DiffRowView, row: Int) {
        let query = findBar.isOpen ? findBar.query : ""
        cell.findQuery = query.isEmpty ? nil : query
        cell.isCurrentFindMatch =
            !findMatches.isEmpty && findCurrent < findMatches.count
            && findMatches[findCurrent] == row
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
        configureFindState(cell, row: row)
        cell.needsDisplay = true
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        false
    }
}

/// Top-left-origin container so overlay math reads like the scroll offsets.
private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}
