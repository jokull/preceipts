// The file-tree sidebar: a source-list NSOutlineView over the Kit's
// FileTree (directory aggregation, chain compaction). Chrome regime —
// semantic colors and SF Symbols throughout; the sidebar material comes
// free from the split item.

import AppKit
import PreceiptsKit

protocol SidebarDelegate: AnyObject {
    func sidebar(_ sidebar: SidebarViewController, didSelectFile fileIndex: Int)
    /// Bubble clicked — open the first thread under this path.
    func sidebar(_ sidebar: SidebarViewController, openThreadsForPath path: String)
}

/// Fetch lifecycle shown in the filter bar.
enum FeedbackLoadState {
    case hidden
    case loading
    case loaded(open: Int, resolved: Int)
}

final class SidebarViewController: NSViewController {
    weak var delegate: SidebarDelegate?
    /// Filter edits land here — the cockpit re-scopes everything.
    var onFilterChange: ((FeedbackFilter) -> Void)?

    private let outline = NSOutlineView()
    private var roots: [FileTreeNode] = []
    /// Leaf nodes by file index, for selection-follows-scroll.
    private var leaves: [Int: FileTreeNode] = [:]
    private var suppressSelectionCallback = false
    /// path → threads-in-scope + drafts anchored in that file.
    private var commentCounts: [String: Int] = [:]

    private let filterBar = NSStackView()
    private let countLabel = NSTextField(labelWithString: "")
    private let spinner = NSProgressIndicator()
    private let authorsControl = NSSegmentedControl(
        labels: ["All", "People", "Bots"], trackingMode: .selectOne, target: nil, action: nil)
    private let resolvedCheckbox = NSButton(
        checkboxWithTitle: "Show resolved", target: nil, action: nil)

    override func loadView() {
        let column = NSTableColumn(identifier: .init("file"))
        column.resizingMask = .autoresizingMask
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.style = .sourceList
        outline.rowSizeStyle = .default
        outline.floatsGroupRows = false
        outline.allowsEmptySelection = true
        outline.autoresizesOutlineColumn = false
        outline.indentationPerLevel = 12
        outline.delegate = self
        outline.dataSource = self
        outline.columnAutoresizingStyle = .uniformColumnAutoresizingStyle

        let scroll = NSScrollView()
        scroll.documentView = outline
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        outline.backgroundColor = .clear

        buildFilterBar()

        let stack = NSStackView(views: [filterBar, scroll])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = Metrics.unit
        NSLayoutConstraint.activate([
            filterBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
        scroll.setContentHuggingPriority(.init(1), for: .vertical)
        self.view = stack
    }

    // ------------------------------------------------------------------
    // Filter bar: the thread scope control. Sits above the tree because
    // what it admits is what the tree's bubbles (and the diff's) show.

    private func buildFilterBar() {
        let title = NSTextField(labelWithString: "Threads")
        title.font = .systemFont(ofSize: 11, weight: .semibold)
        title.textColor = .secondaryLabelColor

        countLabel.font = .systemFont(ofSize: 11)
        countLabel.textColor = .secondaryLabelColor
        countLabel.lineBreakMode = .byTruncatingTail

        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isIndeterminate = true
        spinner.isDisplayedWhenStopped = false

        let titleRow = NSStackView(views: [title, NSView(), countLabel, spinner])
        titleRow.orientation = .horizontal
        titleRow.alignment = .centerY
        titleRow.spacing = Metrics.unit + 2

        authorsControl.controlSize = .small
        authorsControl.segmentDistribution = .fillEqually
        authorsControl.selectedSegment = 0
        authorsControl.target = self
        authorsControl.action = #selector(filterControlChanged(_:))

        resolvedCheckbox.controlSize = .regular
        resolvedCheckbox.font = .systemFont(ofSize: 13)
        resolvedCheckbox.target = self
        resolvedCheckbox.action = #selector(filterControlChanged(_:))

        filterBar.orientation = .vertical
        filterBar.alignment = .leading
        filterBar.spacing = Metrics.padding
        filterBar.edgeInsets = NSEdgeInsets(
            top: Metrics.padding, left: Metrics.paddingWide,
            bottom: Metrics.unit, right: Metrics.paddingWide)
        filterBar.addArrangedSubview(titleRow)
        filterBar.addArrangedSubview(authorsControl)
        filterBar.addArrangedSubview(resolvedCheckbox)
        let inset = 2 * Metrics.paddingWide
        NSLayoutConstraint.activate([
            titleRow.widthAnchor.constraint(equalTo: filterBar.widthAnchor, constant: -inset),
            authorsControl.widthAnchor.constraint(
                equalTo: filterBar.widthAnchor, constant: -inset),
        ])
        filterBar.isHidden = true
    }

    /// Reflect cockpit-owned filter state without firing the callback.
    func setFilter(_ filter: FeedbackFilter) {
        switch filter.authors {
        case .all: authorsControl.selectedSegment = 0
        case .humans: authorsControl.selectedSegment = 1
        case .bots: authorsControl.selectedSegment = 2
        }
        resolvedCheckbox.state = filter.showResolved ? .on : .off
    }

    func setFeedbackState(_ state: FeedbackLoadState) {
        switch state {
        case .hidden:
            filterBar.isHidden = true
            spinner.stopAnimation(nil)
        case .loading:
            filterBar.isHidden = false
            countLabel.stringValue = ""
            spinner.startAnimation(nil)
        case .loaded(let open, let resolved):
            filterBar.isHidden = false
            spinner.stopAnimation(nil)
            var parts = ["\(open) open"]
            if resolved > 0 {
                parts.append("\(resolved) resolved")
            }
            countLabel.stringValue = parts.joined(separator: " \u{00b7} ")
        }
    }

    @objc private func filterControlChanged(_ sender: Any?) {
        let authors: FeedbackFilter.Authors
        switch authorsControl.selectedSegment {
        case 1: authors = .humans
        case 2: authors = .bots
        default: authors = .all
        }
        onFilterChange?(
            FeedbackFilter(authors: authors, showResolved: resolvedCheckbox.state == .on))
    }

    func show(tree: [FileTreeNode]) {
        roots = tree
        leaves.removeAll()
        var stack = tree
        while let node = stack.popLast() {
            if let fileIndex = node.fileIndex {
                leaves[fileIndex] = node
            }
            stack.append(contentsOf: node.children)
        }
        reloadPreservingState()
    }

    /// Feedback bubbles: unresolved threads + drafts per file path.
    func updateCommentCounts(_ counts: [String: Int]) {
        guard counts != commentCounts else { return }
        commentCounts = counts
        reloadPreservingState()
    }

    private func reloadPreservingState() {
        // Watch-driven reloads must not yank the sidebar around: keep the
        // scroll offset and the selection (path-keyed — indices shift).
        let clip = outline.enclosingScrollView?.contentView
        let scrollOrigin = clip?.bounds.origin
        let selectedPath = (outline.item(atRow: outline.selectedRow) as? FileTreeNode)?.path

        outline.reloadData()
        outline.expandItem(nil, expandChildren: true)

        if let selectedPath,
            let node = leaves.values.first(where: { $0.path == selectedPath })
        {
            let row = outline.row(forItem: node)
            if row >= 0 {
                suppressSelectionCallback = true
                outline.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
                suppressSelectionCallback = false
            }
        }
        if let clip, let scrollOrigin {
            outline.layoutSubtreeIfNeeded()
            let maxY = max(0, outline.frame.height - clip.bounds.height)
            clip.scroll(to: NSPoint(x: scrollOrigin.x, y: min(scrollOrigin.y, maxY)))
            outline.enclosingScrollView?.reflectScrolledClipView(clip)
        }
    }

    /// Reflect the surface's current file without echoing back a scroll.
    func highlight(fileIndex: Int) {
        guard let node = leaves[fileIndex] else { return }
        let row = outline.row(forItem: node)
        guard row >= 0, !outline.selectedRowIndexes.contains(row) else { return }
        suppressSelectionCallback = true
        outline.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        outline.scrollRowToVisible(row)
        suppressSelectionCallback = false
    }
}

extension SidebarViewController: NSOutlineViewDataSource, NSOutlineViewDelegate {
    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        guard let node = item as? FileTreeNode else { return roots.count }
        return node.children.count
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        guard let node = item as? FileTreeNode else { return roots[index] }
        return node.children[index]
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        (item as? FileTreeNode)?.isDirectory ?? false
    }

    func outlineView(
        _ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any
    ) -> NSView? {
        guard let node = item as? FileTreeNode else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("tree-cell")
        let cell =
            outline.makeView(withIdentifier: identifier, owner: nil) as? FileTreeCellView
            ?? FileTreeCellView(identifier: identifier)
        cell.configure(node, comments: commentCount(for: node))
        cell.onBubbleClick = { [weak self] in
            guard let self else { return }
            self.delegate?.sidebar(self, openThreadsForPath: node.path)
        }
        return cell
    }

    /// Files read their own count; directories aggregate descendants.
    private func commentCount(for node: FileTreeNode) -> Int {
        if !node.isDirectory {
            return commentCounts[node.path] ?? 0
        }
        let prefix = node.path + "/"
        return commentCounts.reduce(0) { total, entry in
            entry.key.hasPrefix(prefix) ? total + entry.value : total
        }
    }

    func outlineView(_ outlineView: NSOutlineView, shouldSelectItem item: Any) -> Bool {
        (item as? FileTreeNode)?.isDirectory == false
    }

    func outlineViewSelectionDidChange(_ notification: Notification) {
        guard !suppressSelectionCallback,
            let node = outline.item(atRow: outline.selectedRow) as? FileTreeNode,
            let fileIndex = node.fileIndex
        else { return }
        delegate?.sidebar(self, didSelectFile: fileIndex)
    }
}

// One sidebar row: status/folder symbol, name, comment bubble,
// right-aligned ±stats.
private final class FileTreeCellView: NSTableCellView {
    private let icon = NSImageView()
    private let name = NSTextField(labelWithString: "")
    private let bubble = NSButton()
    private let stats = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        name.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
        name.textColor = .labelColor
        name.lineBreakMode = .byTruncatingMiddle
        name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        bubble.isBordered = false
        bubble.imagePosition = .imageLeading
        bubble.font = .monospacedDigitSystemFont(
            ofSize: NSFont.smallSystemFontSize - 1, weight: .medium)
        bubble.contentTintColor = .controlAccentColor
        bubble.setContentHuggingPriority(.required, for: .horizontal)
        bubble.setContentCompressionResistancePriority(.required, for: .horizontal)
        bubble.target = self
        bubble.action = #selector(bubbleClicked(_:))
        bubble.toolTip = "Unresolved feedback \u{2014} click to open"

        stats.font = .monospacedDigitSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .regular)
        stats.setContentHuggingPriority(.required, for: .horizontal)
        stats.setContentCompressionResistancePriority(.required, for: .horizontal)

        for view in [icon, name, bubble, stats] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        imageView = icon
        textField = name

        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            icon.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
            name.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 5),
            name.centerYAnchor.constraint(equalTo: centerYAnchor),
            bubble.leadingAnchor.constraint(
                greaterThanOrEqualTo: name.trailingAnchor, constant: Metrics.padding),
            bubble.centerYAnchor.constraint(equalTo: centerYAnchor),
            stats.leadingAnchor.constraint(
                equalTo: bubble.trailingAnchor, constant: Metrics.unit),
            stats.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -Metrics.unit),
            stats.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var onBubbleClick: (() -> Void)?

    @objc private func bubbleClicked(_ sender: Any?) {
        onBubbleClick?()
    }

    func configure(_ node: FileTreeNode, comments: Int) {
        if comments > 0 {
            bubble.image = NSImage(
                systemSymbolName: "text.bubble.fill", accessibilityDescription: "comments")?
                .withSymbolConfiguration(.init(pointSize: 9, weight: .medium))
            bubble.title = "\(comments)"
            bubble.isHidden = false
        } else {
            bubble.isHidden = true
        }
        configure(node)
    }

    private func configure(_ node: FileTreeNode) {
        name.stringValue = node.name
        if node.isDirectory {
            icon.image = NSImage(systemSymbolName: "folder", accessibilityDescription: "folder")
            icon.contentTintColor = .secondaryLabelColor
        } else {
            let (symbol, tint) = Self.statusGlyph(node.status)
            icon.image = NSImage(
                systemSymbolName: symbol,
                accessibilityDescription: node.status.map(statusName))
            icon.contentTintColor = tint
        }
        let text = NSMutableAttributedString()
        if node.added > 0 {
            text.append(
                NSAttributedString(
                    string: "+\(node.added)",
                    attributes: [.foregroundColor: NSColor.systemGreen, .font: stats.font!]))
        }
        if node.removed > 0 {
            if text.length > 0 {
                text.append(NSAttributedString(string: " ", attributes: [.font: stats.font!]))
            }
            text.append(
                NSAttributedString(
                    string: "\u{2212}\(node.removed)",
                    attributes: [.foregroundColor: NSColor.systemRed, .font: stats.font!]))
        }
        stats.attributedStringValue = text
    }

    private static func statusGlyph(_ status: FileStatus?) -> (String, NSColor) {
        switch status {
        case .added: return ("plus.circle.fill", .systemGreen)
        case .deleted: return ("minus.circle.fill", .systemRed)
        case .renamed: return ("arrow.right.circle.fill", .systemBlue)
        case .modified, .none: return ("pencil.circle.fill", .systemOrange)
        }
    }
}

private func statusName(_ status: FileStatus) -> String {
    switch status {
    case .added: return "added"
    case .modified: return "modified"
    case .deleted: return "deleted"
    case .renamed: return "renamed"
    }
}
