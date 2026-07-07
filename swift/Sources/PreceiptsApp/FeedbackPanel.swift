// The feedback navigator: a compact source-list of PR threads and draft
// comments — files as groups, one row per thread, no bodies. Reading
// happens in the diff via the anchored thread card; this pane answers
// "what's here, where" (docs/desktop-app-design.md "PR feedback viewer").

import AppKit
import PreceiptsKit

enum FeedbackNavItem {
    case thread(FeedbackThread)
    case draft(LocalComment)

    var anchor: (path: String, range: ClosedRange<Int>, side: CommentSide)? {
        switch self {
        case .thread(let thread):
            guard let path = thread.root.path, let range = thread.lineRange else { return nil }
            return (path, range, thread.root.side)
        case .draft(let draft):
            return (draft.path, draft.lineRange, draft.side)
        }
    }

    /// Stable identity for selection sync (thread root id / draft id).
    func matches(_ other: FeedbackNavItem) -> Bool {
        switch (self, other) {
        case (.thread(let a), .thread(let b)):
            return a.root.id == b.root.id && a.root.url == b.root.url
        case (.draft(let a), .draft(let b)):
            return a.id == b.id
        default:
            return false
        }
    }

    var contexts: [CommentContext] {
        switch self {
        case .thread(let thread):
            return thread.comments.map(\.context)
        case .draft(let draft):
            return [
                CommentContext(
                    path: draft.path, line: draft.line, lineText: draft.lineText,
                    body: draft.body, author: nil)
            ]
        }
    }
}

protocol FeedbackPanelDelegate: AnyObject {
    func feedbackPanelRequestsRefresh(_ panel: FeedbackPanelViewController)
    /// Selection changed; nil = deselected. The cockpit opens/closes the
    /// anchored thread card.
    func feedbackPanel(_ panel: FeedbackPanelViewController, didSelect item: FeedbackNavItem?)
    func feedbackPanel(_ panel: FeedbackPanelViewController, deleteDraft id: UInt64)
    /// Nil when the anchor no longer matches the current changeset.
    func feedbackPanel(_ panel: FeedbackPanelViewController, anchorRowFor item: FeedbackNavItem)
        -> Int?
}

final class FeedbackPanelViewController: NSViewController {
    weak var delegate: FeedbackPanelDelegate?

    private var feedback: PrFeedback?
    private var drafts: [LocalComment] = []

    // Filters
    private var authorFilter: AuthorFilter = .all
    private var showOutdated = true

    private enum AuthorFilter: Int {
        case all, humans, bots
    }

    // Outline model: groups of rows.
    private final class Group {
        let title: String
        var items: [Row]
        init(title: String, items: [Row]) {
            self.title = title
            self.items = items
        }
    }

    private final class Row {
        let item: FeedbackNavItem
        let outdated: Bool
        init(item: FeedbackNavItem, outdated: Bool) {
            self.item = item
            self.outdated = outdated
        }
    }

    private var groups: [Group] = []

    private let prTitle = NSTextField(labelWithString: "Feedback")
    private let outline = NSOutlineView()
    private let statusLabel = NSTextField(wrappingLabelWithString: "No feedback loaded")
    private var suppressSelection = false

    // ------------------------------------------------------------------
    // View construction

    override func loadView() {
        prTitle.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        prTitle.lineBreakMode = .byTruncatingTail

        let refresh = NSButton()
        refresh.image = NSImage(
            systemSymbolName: "arrow.clockwise", accessibilityDescription: "refresh")
        refresh.isBordered = false
        refresh.controlSize = .small
        refresh.target = self
        refresh.action = #selector(refreshClicked(_:))
        refresh.toolTip = "Fetch PR feedback"

        let copyAll = NSButton()
        copyAll.image = NSImage(
            systemSymbolName: "doc.on.doc", accessibilityDescription: "copy all")
        copyAll.isBordered = false
        copyAll.controlSize = .small
        copyAll.target = self
        copyAll.action = #selector(copyAllClicked(_:))
        copyAll.toolTip = "Copy all visible comments as a by-file digest"

        let filter = NSPopUpButton()
        filter.pullsDown = true
        filter.isBordered = false
        filter.controlSize = .small
        let filterMenu = NSMenu()
        let iconItem = NSMenuItem()
        iconItem.image = NSImage(
            systemSymbolName: "line.3.horizontal.decrease.circle",
            accessibilityDescription: "filter")
        filterMenu.addItem(iconItem)  // pull-down title slot
        for (title, tag) in [("All Authors", 0), ("Humans", 1), ("Bots", 2)] {
            let item = NSMenuItem(
                title: title, action: #selector(authorFilterChanged(_:)), keyEquivalent: "")
            item.target = self
            item.tag = tag
            filterMenu.addItem(item)
        }
        filterMenu.addItem(.separator())
        let outdatedItem = NSMenuItem(
            title: "Show Outdated", action: #selector(toggleOutdated(_:)), keyEquivalent: "")
        outdatedItem.target = self
        filterMenu.addItem(outdatedItem)
        filter.menu = filterMenu
        filter.toolTip = "Filter feedback"

        let header = NSStackView(views: [prTitle, NSView(), filter, copyAll, refresh])
        header.orientation = .horizontal
        header.spacing = Metrics.unit
        header.edgeInsets = NSEdgeInsets(
            top: Metrics.padding, left: Metrics.paddingWide,
            bottom: 0, right: Metrics.padding)

        let column = NSTableColumn(identifier: .init("nav"))
        column.resizingMask = .autoresizingMask
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.style = .sourceList
        outline.rowSizeStyle = .small
        outline.floatsGroupRows = false
        outline.indentationPerLevel = 4
        outline.autoresizesOutlineColumn = false
        outline.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        outline.allowsEmptySelection = true
        outline.delegate = self
        outline.dataSource = self
        outline.menu = makeContextMenu()

        let scroll = NSScrollView()
        scroll.documentView = outline
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        outline.backgroundColor = .clear

        statusLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        statusLabel.textColor = .secondaryLabelColor

        let statusWrap = NSStackView(views: [statusLabel])
        statusWrap.edgeInsets = NSEdgeInsets(
            top: 0, left: Metrics.paddingWide, bottom: Metrics.padding,
            right: Metrics.paddingWide)

        let stack = NSStackView(views: [header, scroll, statusWrap])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = Metrics.unit
        scroll.setContentHuggingPriority(.init(1), for: .vertical)
        self.view = stack
        NSLayoutConstraint.activate([
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor),
            statusWrap.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    private func makeContextMenu() -> NSMenu {
        let menu = NSMenu()
        menu.addItem(
            withTitle: "Copy", action: #selector(copyRowClicked(_:)), keyEquivalent: ""
        ).target = self
        menu.addItem(
            withTitle: "Open on GitHub", action: #selector(openRowClicked(_:)),
            keyEquivalent: ""
        ).target = self
        menu.addItem(
            withTitle: "Delete Draft", action: #selector(deleteRowClicked(_:)),
            keyEquivalent: ""
        ).target = self
        return menu
    }

    // ------------------------------------------------------------------
    // State

    func apply(feedback: PrFeedback?, drafts: [LocalComment]) {
        self.feedback = feedback
        self.drafts = drafts
        if let feedback {
            prTitle.stringValue = "#\(feedback.number) \(feedback.title)"
            prTitle.toolTip = feedback.url
        }
        rebuild()
    }

    func showStatus(_ message: String) {
        statusLabel.stringValue = message
        statusLabel.isHidden = false
    }

    /// Deselect without echoing back (card closed from the diff side).
    func clearSelection() {
        suppressSelection = true
        outline.deselectAll(nil)
        suppressSelection = false
    }

    /// Reflect a thread opened from the diff side, without echoing back.
    func highlight(_ item: FeedbackNavItem) {
        for group in groups {
            for row in group.items where row.item.matches(item) {
                let index = outline.row(forItem: row)
                guard index >= 0 else { continue }
                suppressSelection = true
                outline.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
                outline.scrollRowToVisible(index)
                suppressSelection = false
                return
            }
        }
    }

    private func rebuild() {
        var fileGroups: [String: [Row]] = [:]
        var conversation: [Row] = []

        for thread in FeedbackThread.group(feedback?.comments ?? []) {
            switch authorFilter {
            case .humans where thread.root.isBot: continue
            case .bots where !thread.root.isBot: continue
            default: break
            }
            let item = FeedbackNavItem.thread(thread)
            let outdated =
                thread.root.outdated
                || (item.anchor != nil
                    && delegate?.feedbackPanel(self, anchorRowFor: item) == nil)
            if outdated, !showOutdated { continue }
            let row = Row(item: item, outdated: outdated)
            if let path = thread.root.path {
                fileGroups[path, default: []].append(row)
            } else {
                conversation.append(row)
            }
        }

        groups = []
        if !drafts.isEmpty {
            groups.append(
                Group(
                    title: "Drafts",
                    items: drafts.map { draft in
                        let item = FeedbackNavItem.draft(draft)
                        let outdated =
                            delegate?.feedbackPanel(self, anchorRowFor: item) == nil
                        return Row(item: item, outdated: outdated)
                    }))
        }
        for path in fileGroups.keys.sorted() {
            groups.append(Group(title: path, items: fileGroups[path]!))
        }
        if !conversation.isEmpty {
            groups.append(Group(title: "Conversation", items: conversation))
        }

        suppressSelection = true
        outline.reloadData()
        outline.expandItem(nil, expandChildren: true)
        suppressSelection = false

        let count = groups.reduce(0) { $0 + $1.items.count }
        statusLabel.isHidden = count > 0
        if count == 0 {
            statusLabel.stringValue =
                feedback == nil ? "No feedback loaded" : "Nothing matches the filters"
        }
    }

    // ------------------------------------------------------------------
    // Actions

    @objc private func refreshClicked(_ sender: Any?) {
        showStatus("Fetching PR feedback\u{2026}")
        delegate?.feedbackPanelRequestsRefresh(self)
    }

    @objc private func authorFilterChanged(_ sender: NSMenuItem) {
        authorFilter = AuthorFilter(rawValue: sender.tag) ?? .all
        sender.menu?.items.forEach { item in
            if item.action == #selector(authorFilterChanged(_:)) {
                item.state = item.tag == sender.tag ? .on : .off
            }
        }
        rebuild()
    }

    @objc private func toggleOutdated(_ sender: NSMenuItem) {
        showOutdated.toggle()
        sender.state = showOutdated ? .on : .off
        rebuild()
    }

    @objc private func copyAllClicked(_ sender: Any?) {
        let contexts = groups.flatMap(\.items).flatMap(\.item.contexts)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(formatDigest(contexts), forType: .string)
    }

    private var clickedRow: Row? {
        outline.item(atRow: outline.clickedRow) as? Row
    }

    @objc private func copyRowClicked(_ sender: Any?) {
        guard let row = clickedRow else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(
            row.item.contexts.map(formatComment).joined(separator: "\n"), forType: .string)
    }

    @objc private func openRowClicked(_ sender: Any?) {
        guard let row = clickedRow, case .thread(let thread) = row.item,
            let url = URL(string: thread.root.url)
        else { return }
        NSWorkspace.shared.open(url)
    }

    @objc private func deleteRowClicked(_ sender: Any?) {
        guard let row = clickedRow, case .draft(let draft) = row.item else { return }
        delegate?.feedbackPanel(self, deleteDraft: draft.id)
    }
}

// ------------------------------------------------------------------
// Outline

extension FeedbackPanelViewController: NSOutlineViewDataSource, NSOutlineViewDelegate {
    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        if item == nil { return groups.count }
        return (item as? Group)?.items.count ?? 0
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        if let group = item as? Group { return group.items[index] }
        return groups[index]
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        item is Group
    }

    func outlineView(_ outlineView: NSOutlineView, isGroupItem item: Any) -> Bool {
        item is Group
    }

    func outlineView(_ outlineView: NSOutlineView, shouldSelectItem item: Any) -> Bool {
        item is Row
    }

    func outlineView(
        _ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any
    ) -> NSView? {
        if let group = item as? Group {
            let identifier = NSUserInterfaceItemIdentifier("nav-group")
            let cell =
                outline.makeView(withIdentifier: identifier, owner: nil) as? NSTableCellView
                ?? {
                    let cell = NSTableCellView()
                    cell.identifier = identifier
                    let label = NSTextField(labelWithString: "")
                    label.font = .systemFont(
                        ofSize: NSFont.smallSystemFontSize - 1, weight: .semibold)
                    label.textColor = .secondaryLabelColor
                    label.lineBreakMode = .byTruncatingHead
                    label.translatesAutoresizingMaskIntoConstraints = false
                    cell.addSubview(label)
                    cell.textField = label
                    NSLayoutConstraint.activate([
                        label.leadingAnchor.constraint(equalTo: cell.leadingAnchor),
                        label.trailingAnchor.constraint(
                            lessThanOrEqualTo: cell.trailingAnchor),
                        label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                    ])
                    return cell
                }()
            cell.textField?.stringValue = group.title
            cell.textField?.toolTip = group.title
            return cell
        }
        guard let row = item as? Row else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("nav-row")
        let cell =
            outline.makeView(withIdentifier: identifier, owner: nil) as? FeedbackNavCellView
            ?? FeedbackNavCellView(identifier: identifier)
        cell.configure(row.item, outdated: row.outdated)
        return cell
    }

    func outlineViewSelectionDidChange(_ notification: Notification) {
        guard !suppressSelection else { return }
        let row = outline.item(atRow: outline.selectedRow) as? Row
        delegate?.feedbackPanel(self, didSelect: row?.item)
    }
}

// One navigator row: glyph · author · L-range · reply count. No bodies.
private final class FeedbackNavCellView: NSTableCellView {
    private let icon = NSImageView()
    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        title.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        title.textColor = .labelColor
        title.lineBreakMode = .byTruncatingTail
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        detail.font = .monospacedDigitSystemFont(
            ofSize: NSFont.smallSystemFontSize - 1, weight: .regular)
        detail.textColor = .secondaryLabelColor
        detail.setContentHuggingPriority(.required, for: .horizontal)
        detail.setContentCompressionResistancePriority(.required, for: .horizontal)

        for view in [icon, title, detail] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        imageView = icon
        textField = title
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            icon.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 14),
            title.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 5),
            title.centerYAnchor.constraint(equalTo: centerYAnchor),
            detail.leadingAnchor.constraint(
                greaterThanOrEqualTo: title.trailingAnchor, constant: Metrics.padding),
            detail.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -Metrics.unit),
            detail.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func configure(_ item: FeedbackNavItem, outdated: Bool) {
        switch item {
        case .draft(let draft):
            icon.image = NSImage(
                systemSymbolName: "square.and.pencil", accessibilityDescription: "draft")
            icon.contentTintColor = .controlAccentColor
            title.stringValue = firstLine(draft.body)
            let range = draft.lineRange
            detail.stringValue =
                range.count == 1
                ? "L\(range.lowerBound)"
                : "L\(range.lowerBound)\u{2013}\(range.upperBound)"
        case .thread(let thread):
            let (symbol, tint) = Self.glyph(thread.root, outdated: outdated)
            icon.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            icon.contentTintColor = tint
            title.stringValue = thread.root.author
            var parts: [String] = []
            if let range = thread.lineRange {
                parts.append(
                    range.count == 1
                        ? "L\(range.lowerBound)"
                        : "L\(range.lowerBound)\u{2013}\(range.upperBound)")
            }
            if !thread.replies.isEmpty {
                parts.append("\u{21a9}\(thread.replies.count)")
            }
            if let state = thread.root.state, !state.isEmpty {
                parts.append(state.lowercased().replacingOccurrences(of: "_", with: " "))
            }
            detail.stringValue = parts.joined(separator: "  ")
        }
        detail.textColor = outdated ? .systemOrange : .secondaryLabelColor
        toolTip = firstLine(item.contexts.first?.body ?? "")
    }

    private func firstLine(_ text: String) -> String {
        text.split(separator: "\n").first.map(String.init) ?? text
    }

    private static func glyph(
        _ comment: FeedbackComment, outdated: Bool
    ) -> (String, NSColor) {
        if outdated {
            return ("clock.arrow.circlepath", .systemOrange)
        }
        switch comment.kind {
        case .reviewComment:
            return (
                comment.isBot ? "gearshape.2" : "text.bubble",
                .secondaryLabelColor
            )
        case .review:
            switch comment.state {
            case "APPROVED": return ("checkmark.seal.fill", .systemGreen)
            case "CHANGES_REQUESTED": return ("exclamationmark.octagon.fill", .systemRed)
            default: return ("checkmark.message", .secondaryLabelColor)
            }
        case .conversation:
            return ("bubble.left.and.bubble.right", .secondaryLabelColor)
        }
    }
}
