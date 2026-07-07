// The feedback panel: PR review comments, review verdicts, conversation
// comments (humans and bots), and local draft comments — one list with
// filters and the copy affordances that feed coding agents
// (docs/desktop-app-design.md "PR feedback viewer"). Chrome regime.

import AppKit
import PreceiptsKit

enum FeedbackItem {
    case github(FeedbackComment)
    case draft(LocalComment)

    var context: CommentContext {
        switch self {
        case .github(let comment):
            return comment.context
        case .draft(let draft):
            return CommentContext(
                path: draft.path, line: draft.line, lineText: draft.lineText,
                body: draft.body, author: nil)
        }
    }

    var anchor: (path: String, line: Int, side: CommentSide)? {
        switch self {
        case .github(let comment):
            guard let path = comment.path, let line = comment.line else { return nil }
            return (path, line, .new)
        case .draft(let draft):
            return (draft.path, draft.line, draft.side)
        }
    }
}

protocol FeedbackPanelDelegate: AnyObject {
    func feedbackPanelRequestsRefresh(_ panel: FeedbackPanelViewController)
    func feedbackPanel(_ panel: FeedbackPanelViewController, scrollTo item: FeedbackItem)
    func feedbackPanel(_ panel: FeedbackPanelViewController, deleteDraft id: UInt64)
    /// Nil when the anchor no longer matches the current changeset.
    func feedbackPanel(_ panel: FeedbackPanelViewController, anchorRowFor item: FeedbackItem)
        -> Int?
}

final class FeedbackPanelViewController: NSViewController {
    weak var delegate: FeedbackPanelDelegate?

    private var feedback: PrFeedback?
    private var drafts: [LocalComment] = []
    private var visible: [FeedbackItem] = []

    private let title_ = NSTextField(labelWithString: "Feedback")
    private let refreshButton = NSButton()
    private let authorFilter = NSSegmentedControl(
        labels: ["All", "Humans", "Bots"], trackingMode: .selectOne, target: nil, action: nil)
    private let outdatedToggle = NSButton(
        checkboxWithTitle: "Outdated", target: nil, action: nil)
    private let copyAllButton = NSButton()
    private let table = NSTableView()
    private let emptyLabel = NSTextField(wrappingLabelWithString: "")

    // ------------------------------------------------------------------
    // View construction

    override func loadView() {
        title_.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        title_.textColor = .labelColor
        title_.lineBreakMode = .byTruncatingTail

        refreshButton.image = NSImage(
            systemSymbolName: "arrow.clockwise", accessibilityDescription: "refresh")
        refreshButton.bezelStyle = .accessoryBarAction
        refreshButton.controlSize = .small
        refreshButton.target = self
        refreshButton.action = #selector(refreshClicked(_:))
        refreshButton.toolTip = "Fetch PR feedback (gh)"

        authorFilter.selectedSegment = 0
        authorFilter.controlSize = .small
        authorFilter.target = self
        authorFilter.action = #selector(filtersChanged(_:))

        outdatedToggle.state = .on
        outdatedToggle.controlSize = .small
        outdatedToggle.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        outdatedToggle.target = self
        outdatedToggle.action = #selector(filtersChanged(_:))
        outdatedToggle.toolTip = "Show comments whose anchor no longer matches the diff"

        copyAllButton.title = "Copy All"
        copyAllButton.image = NSImage(
            systemSymbolName: "doc.on.doc", accessibilityDescription: "copy all")
        copyAllButton.imagePosition = .imageLeading
        copyAllButton.bezelStyle = .accessoryBarAction
        copyAllButton.controlSize = .small
        copyAllButton.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        copyAllButton.target = self
        copyAllButton.action = #selector(copyAllClicked(_:))
        copyAllButton.toolTip = "Copy every visible comment as a by-file digest"

        let headerRow = NSStackView(views: [title_, NSView(), refreshButton])
        headerRow.orientation = .horizontal
        headerRow.spacing = Metrics.padding
        let filterRow = NSStackView(views: [authorFilter, outdatedToggle, NSView(), copyAllButton])
        filterRow.orientation = .horizontal
        filterRow.spacing = Metrics.padding

        let column = NSTableColumn(identifier: .init("feedback"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.usesAutomaticRowHeights = true
        table.style = .inset
        table.selectionHighlightStyle = .regular
        table.intercellSpacing = NSSize(width: 0, height: 6)
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.delegate = self
        table.dataSource = self
        table.action = #selector(rowClicked(_:))
        table.target = self

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        table.backgroundColor = .clear

        emptyLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.stringValue = "No feedback loaded"

        let stack = NSStackView(views: [headerRow, filterRow, scroll, emptyLabel])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = Metrics.padding
        stack.edgeInsets = NSEdgeInsets(
            top: Metrics.padding, left: Metrics.paddingWide,
            bottom: Metrics.padding, right: Metrics.paddingWide)
        scroll.setContentHuggingPriority(.init(1), for: .vertical)

        self.view = stack
        NSLayoutConstraint.activate([
            headerRow.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingWide),
            filterRow.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingWide),
            scroll.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingWide),
        ])
    }

    // ------------------------------------------------------------------
    // State from the cockpit

    func apply(feedback: PrFeedback?, drafts: [LocalComment]) {
        self.feedback = feedback
        self.drafts = drafts
        if let feedback {
            title_.stringValue = "#\(feedback.number) \(feedback.title)"
            title_.toolTip = feedback.url
        }
        rebuild()
    }

    func showStatus(_ message: String) {
        emptyLabel.stringValue = message
        emptyLabel.isHidden = false
    }

    private func rebuild() {
        var items: [FeedbackItem] = drafts.map { .draft($0) }
        if let feedback {
            items += feedback.comments.map { FeedbackItem.github($0) }
        }
        visible = items.filter { item in
            switch item {
            case .draft:
                return true
            case .github(let comment):
                switch authorFilter.selectedSegment {
                case 1 where comment.isBot: return false
                case 2 where !comment.isBot: return false
                default: break
                }
                if outdatedToggle.state == .off, isOutdated(item) {
                    return false
                }
                return true
            }
        }
        table.reloadData()
        emptyLabel.isHidden = !visible.isEmpty
        if visible.isEmpty {
            emptyLabel.stringValue =
                feedback == nil
                ? "No feedback loaded"
                : "No comments match the current filters"
        }
    }

    /// GitHub says outdated via nulled line; drafts (and github rows) are
    /// also outdated when their anchor no longer resolves in this diff.
    private func isOutdated(_ item: FeedbackItem) -> Bool {
        if case .github(let comment) = item, comment.outdated {
            return true
        }
        guard item.anchor != nil else { return false }
        return delegate?.feedbackPanel(self, anchorRowFor: item) == nil
    }

    // ------------------------------------------------------------------
    // Actions

    @objc private func refreshClicked(_ sender: Any?) {
        showStatus("Fetching PR feedback\u{2026}")
        delegate?.feedbackPanelRequestsRefresh(self)
    }

    @objc private func filtersChanged(_ sender: Any?) {
        rebuild()
    }

    @objc private func copyAllClicked(_ sender: Any?) {
        let digest = formatDigest(visible.map(\.context))
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(digest, forType: .string)
    }

    @objc private func rowClicked(_ sender: Any?) {
        let row = table.clickedRow
        guard row >= 0, row < visible.count else { return }
        delegate?.feedbackPanel(self, scrollTo: visible[row])
    }

    fileprivate func copy(_ item: FeedbackItem) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(formatComment(item.context), forType: .string)
    }

    fileprivate func delete(_ item: FeedbackItem) {
        if case .draft(let draft) = item {
            delegate?.feedbackPanel(self, deleteDraft: draft.id)
        }
    }
}

// ------------------------------------------------------------------
// Table

extension FeedbackPanelViewController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int {
        visible.count
    }

    func tableView(
        _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
    ) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("feedback-cell")
        let cell =
            table.makeView(withIdentifier: identifier, owner: nil) as? FeedbackCellView
            ?? FeedbackCellView(identifier: identifier)
        let item = visible[row]
        cell.configure(item, outdated: isOutdated(item))
        cell.onCopy = { [weak self] in self?.copy(item) }
        cell.onDelete = { [weak self] in self?.delete(item) }
        return cell
    }
}

// One comment: header line (icon, author, badges, anchor), body, actions.
private final class FeedbackCellView: NSTableCellView {
    var onCopy: (() -> Void)?
    var onDelete: (() -> Void)?

    private let icon = NSImageView()
    private let author = NSTextField(labelWithString: "")
    private let badge = NSTextField(labelWithString: "")
    private let anchor = NSTextField(labelWithString: "")
    private let body = NSTextField(wrappingLabelWithString: "")
    private let copyButton = NSButton()
    private let deleteButton = NSButton()

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        author.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        badge.font = .systemFont(ofSize: NSFont.smallSystemFontSize - 1)
        badge.textColor = .secondaryLabelColor
        anchor.font = .monospacedSystemFont(
            ofSize: NSFont.smallSystemFontSize - 1, weight: .regular)
        anchor.textColor = .secondaryLabelColor
        anchor.lineBreakMode = .byTruncatingHead

        body.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        body.textColor = .labelColor
        body.maximumNumberOfLines = 12
        body.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        copyButton.image = NSImage(
            systemSymbolName: "doc.on.doc", accessibilityDescription: "copy")
        copyButton.isBordered = false
        copyButton.controlSize = .small
        copyButton.target = self
        copyButton.action = #selector(copyClicked(_:))
        copyButton.toolTip = "Copy as a markdown block"

        deleteButton.image = NSImage(
            systemSymbolName: "trash", accessibilityDescription: "delete draft")
        deleteButton.isBordered = false
        deleteButton.controlSize = .small
        deleteButton.target = self
        deleteButton.action = #selector(deleteClicked(_:))
        deleteButton.toolTip = "Delete this draft"

        let header = NSStackView(views: [
            icon, author, badge, anchor, NSView(), copyButton, deleteButton,
        ])
        header.orientation = .horizontal
        header.spacing = 5
        header.alignment = .centerY

        let stack = NSStackView(views: [header, body])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 3
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 2),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -2),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -2),
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            body.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func configure(_ item: FeedbackItem, outdated: Bool) {
        switch item {
        case .draft(let draft):
            icon.image = NSImage(
                systemSymbolName: "square.and.pencil", accessibilityDescription: "draft")
            icon.contentTintColor = .controlAccentColor
            author.stringValue = "Draft"
            badge.stringValue = "never posted"
            anchor.stringValue = "\(draft.path):\(draft.line)"
            body.stringValue = draft.body
            deleteButton.isHidden = false
        case .github(let comment):
            let (symbol, tint) = Self.glyph(comment)
            icon.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            icon.contentTintColor = tint
            author.stringValue = comment.author
            var badges: [String] = []
            if comment.isBot { badges.append("bot") }
            if let state = comment.state, !state.isEmpty {
                badges.append(state.lowercased().replacingOccurrences(of: "_", with: " "))
            }
            if outdated { badges.append("outdated") }
            badge.stringValue = badges.joined(separator: " \u{00b7} ")
            badge.textColor = outdated ? .systemOrange : .secondaryLabelColor
            if let path = comment.path, let line = comment.line {
                anchor.stringValue = "\(path):\(line)"
            } else {
                anchor.stringValue = ""
            }
            body.stringValue = comment.body
            deleteButton.isHidden = true
        }
        body.toolTip = body.stringValue
    }

    private static func glyph(_ comment: FeedbackComment) -> (String, NSColor) {
        switch comment.kind {
        case .reviewComment:
            return ("text.bubble", .secondaryLabelColor)
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

    @objc private func copyClicked(_ sender: Any?) {
        onCopy?()
    }

    @objc private func deleteClicked(_ sender: Any?) {
        onDelete?()
    }
}
