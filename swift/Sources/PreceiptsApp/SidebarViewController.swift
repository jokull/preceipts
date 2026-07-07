// The file-tree sidebar: a source-list NSOutlineView over the Kit's
// FileTree (directory aggregation, chain compaction). Chrome regime —
// semantic colors and SF Symbols throughout; the sidebar material comes
// free from the split item.

import AppKit
import PreceiptsKit

protocol SidebarDelegate: AnyObject {
    func sidebar(_ sidebar: SidebarViewController, didSelectFile fileIndex: Int)
}

final class SidebarViewController: NSViewController {
    weak var delegate: SidebarDelegate?

    private let outline = NSOutlineView()
    private var roots: [FileTreeNode] = []
    /// Leaf nodes by file index, for selection-follows-scroll.
    private var leaves: [Int: FileTreeNode] = [:]
    private var suppressSelectionCallback = false

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
        self.view = scroll
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
        outline.reloadData()
        outline.expandItem(nil, expandChildren: true)
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
        cell.configure(node)
        return cell
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

// One sidebar row: status/folder symbol, name, right-aligned ±stats.
private final class FileTreeCellView: NSTableCellView {
    private let icon = NSImageView()
    private let name = NSTextField(labelWithString: "")
    private let stats = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        name.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
        name.textColor = .labelColor
        name.lineBreakMode = .byTruncatingMiddle
        name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        stats.font = .monospacedDigitSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .regular)
        stats.setContentHuggingPriority(.required, for: .horizontal)
        stats.setContentCompressionResistancePriority(.required, for: .horizontal)

        for view in [icon, name, stats] as [NSView] {
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
            stats.leadingAnchor.constraint(
                greaterThanOrEqualTo: name.trailingAnchor, constant: Metrics.padding),
            stats.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -Metrics.unit),
            stats.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func configure(_ node: FileTreeNode) {
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
