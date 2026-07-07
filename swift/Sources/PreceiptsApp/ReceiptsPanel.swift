// The receipts panel (⌘J) — the TUI rail, grown up: prepare steps +
// checks with state/duration/runner, disclosure rows streaming live
// output during `run --events`, a run button, and the run-level note
// line (normalized / invalidated / lock refusal). Chrome regime.

import AppKit
import PreceiptsKit

final class ReceiptsPanelView: NSView {
    var onRunClicked: (() -> Void)?

    private let outline = NSOutlineView()
    private let runButton = NSButton()
    private let note = NSTextField(labelWithString: "")
    private var rows: [ReceiptRowItem] = []

    // ------------------------------------------------------------------
    // Construction

    init() {
        super.init(frame: .zero)

        let header = makeHeader()

        let column = NSTableColumn(identifier: .init("receipt"))
        column.resizingMask = .autoresizingMask
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.style = .fullWidth
        outline.rowSizeStyle = .custom
        outline.indentationPerLevel = 8
        outline.autoresizesOutlineColumn = false
        outline.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        outline.allowsEmptySelection = true
        outline.selectionHighlightStyle = .none
        outline.delegate = self
        outline.dataSource = self

        let scroll = NSScrollView()
        scroll.documentView = outline
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = true
        scroll.backgroundColor = .controlBackgroundColor

        let stack = NSStackView(views: [header, scroll])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            header.heightAnchor.constraint(equalToConstant: 30),
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func makeHeader() -> NSView {
        let bar = NSVisualEffectView()
        bar.material = .headerView
        bar.blendingMode = .withinWindow

        let topSeparator = NSBox()
        topSeparator.boxType = .separator
        let bottomSeparator = NSBox()
        bottomSeparator.boxType = .separator

        runButton.title = "Run Checks"
        runButton.image = NSImage(systemSymbolName: "play.fill", accessibilityDescription: "run")
        runButton.imagePosition = .imageLeading
        runButton.bezelStyle = .accessoryBarAction
        runButton.controlSize = .small
        runButton.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        runButton.target = self
        runButton.action = #selector(runClicked(_:))
        runButton.toolTip = "Run checks via preceipts-engine (\u{2318}R)"

        note.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        note.textColor = .secondaryLabelColor
        note.lineBreakMode = .byTruncatingTail

        for view in [topSeparator, bottomSeparator, runButton, note] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            bar.addSubview(view)
        }
        NSLayoutConstraint.activate([
            topSeparator.topAnchor.constraint(equalTo: bar.topAnchor),
            topSeparator.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
            topSeparator.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
            bottomSeparator.bottomAnchor.constraint(equalTo: bar.bottomAnchor),
            bottomSeparator.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
            bottomSeparator.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
            runButton.leadingAnchor.constraint(
                equalTo: bar.leadingAnchor, constant: Metrics.padding),
            runButton.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
            note.leadingAnchor.constraint(
                equalTo: runButton.trailingAnchor, constant: Metrics.paddingWide),
            note.trailingAnchor.constraint(
                lessThanOrEqualTo: bar.trailingAnchor, constant: -Metrics.paddingWide),
            note.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
        ])
        return bar
    }

    @objc private func runClicked(_ sender: Any?) {
        onRunClicked?()
    }

    // ------------------------------------------------------------------
    // State from the cockpit

    /// At-rest table from the hud's status (between runs).
    func apply(status: EngineStatus) {
        // A live run owns the table; the post-run hud refresh lands after.
        guard !rows.contains(where: { $0.state == .running }) else { return }
        rows = status.rows.map { row in
            let item = ReceiptRowItem(kind: .check, name: row.check)
            item.required = row.required
            switch row.state {
            case .ok: item.state = .ok
            case .fail: item.state = .fail
            case .missing: item.state = .missing
            case .staleDefinition: item.state = .staleDefinition
            }
            if let receipt = row.receipt {
                var parts = [
                    formatDuration(receipt.durationMs),
                    receipt.runner.email.isEmpty ? receipt.runner.name : receipt.runner.email,
                ]
                if let agent = receipt.runner.agent {
                    parts.append("(\(agent))")
                }
                item.detail = parts.joined(separator: " \u{00b7} ")
            }
            return item
        }
        outline.reloadData()
    }

    func beginRun() {
        note.stringValue = ""
        for row in rows where row.kind == .check {
            row.state = .pending
            row.clearOutput()
        }
        rows.removeAll { $0.kind == .prepare }
        outline.reloadData()
    }

    func endRun(_ outcome: RunOutcome) {
        for row in rows where row.state == .running {
            row.state = outcome.exit == 0 ? .ok : .fail
        }
        // Nonzero-beyond-failure exits are engine refusals (lock, invalid
        // run) — surface their explanation if events didn't already.
        if note.stringValue.isEmpty, outcome.exit != 0, outcome.exit != 1,
            let text = outcome.note
        {
            note.stringValue = text.components(separatedBy: .newlines).first ?? text
            note.toolTip = text
        }
        outline.reloadData()
    }

    func handle(_ event: EngineRunEvent) {
        switch event {
        case .prepareStarted(let step):
            let item = findOrInsert(kind: .prepare, name: step)
            item.state = .running
            outline.reloadData()
        case .prepareOutput(let step, let chunk):
            find(kind: .prepare, name: step)?.appendOutput(chunk)
        case .prepareFinished(let step, let ok, _, let durationMs):
            if let item = find(kind: .prepare, name: step) {
                item.state = ok ? .ok : .fail
                item.detail = formatDuration(durationMs)
                reload(item)
            }
        case .treeNormalized(_, let changed):
            note.stringValue =
                "Prepare normalized the worktree \u{2014} \(changed.count) file(s); receipts key to the normalized tree"
        case .runStarted:
            break
        case .worktreeChanged(_, _, let changed):
            note.stringValue =
                "Worktree changed while checks ran (\(changed.count) file(s)) \u{2014} no receipts minted"
            note.toolTip =
                "If a check formats or generates code, move that command to [prepare] in .preceipts/config.toml"
        case .checkStarted(let check, _):
            let item = findOrInsert(kind: .check, name: check)
            item.state = .running
            item.clearOutput()
            reload(item)
        case .output(let check, let chunk):
            find(kind: .check, name: check)?.appendOutput(chunk)
        case .checkFinished(let check, let ok, _, let durationMs):
            if let item = find(kind: .check, name: check) {
                item.state = ok ? .ok : .fail
                item.detail = formatDuration(durationMs)
                reload(item)
            }
        case .receiptMinted:
            break
        }
    }

    private func find(kind: ReceiptRowItem.Kind, name: String) -> ReceiptRowItem? {
        rows.first { $0.kind == kind && $0.name == name }
    }

    private func findOrInsert(kind: ReceiptRowItem.Kind, name: String) -> ReceiptRowItem {
        if let existing = find(kind: kind, name: name) {
            return existing
        }
        let item = ReceiptRowItem(kind: kind, name: name)
        if kind == .prepare {
            let checksStart = rows.firstIndex { $0.kind == .check } ?? rows.count
            rows.insert(item, at: checksStart)
        } else {
            rows.append(item)
        }
        outline.reloadData()
        return item
    }

    private func reload(_ item: ReceiptRowItem) {
        outline.reloadItem(item, reloadChildren: false)
    }
}

// ------------------------------------------------------------------
// Rows

final class ReceiptRowItem {
    enum Kind { case prepare, check }
    enum State { case pending, running, ok, fail, missing, staleDefinition }

    let kind: Kind
    let name: String
    var state: State = .pending
    var detail = ""
    var required = false
    /// Child item for the disclosure row; its text view accumulates the
    /// streamed output directly (no reload churn on chatty logs).
    let output = ReceiptOutputItem()

    init(kind: Kind, name: String) {
        self.kind = kind
        self.name = name
        output.parent = self
    }

    func appendOutput(_ chunk: String) {
        output.append(chunk)
    }

    func clearOutput() {
        output.clear()
    }
}

final class ReceiptOutputItem {
    weak var parent: ReceiptRowItem?
    let textView: NSTextView
    let scroll: NSScrollView

    init() {
        scroll = NSTextView.scrollableTextView()
        textView = scroll.documentView as! NSTextView
        textView.isEditable = false
        textView.font = .monospacedSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .regular)
        textView.backgroundColor = .textBackgroundColor
        textView.textContainerInset = NSSize(width: 6, height: 6)
        scroll.hasVerticalScroller = true
        scroll.borderType = .noBorder
    }

    func append(_ chunk: String) {
        textView.textStorage?.append(
            NSAttributedString(
                string: chunk,
                attributes: [
                    .font: textView.font ?? NSFont.monospacedSystemFont(
                        ofSize: NSFont.smallSystemFontSize, weight: .regular),
                    .foregroundColor: NSColor.labelColor,
                ]))
        textView.scrollToEndOfDocument(nil)
    }

    func clear() {
        textView.string = ""
    }
}

extension ReceiptsPanelView: NSOutlineViewDataSource, NSOutlineViewDelegate {
    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        if item == nil { return rows.count }
        return item is ReceiptRowItem ? 1 : 0
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        if let row = item as? ReceiptRowItem { return row.output }
        return rows[index]
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        item is ReceiptRowItem
    }

    func outlineView(_ outlineView: NSOutlineView, heightOfRowByItem item: Any) -> CGFloat {
        item is ReceiptOutputItem ? 140 : 24
    }

    func outlineView(
        _ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any
    ) -> NSView? {
        if let output = item as? ReceiptOutputItem {
            return output.scroll
        }
        guard let row = item as? ReceiptRowItem else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("receipt-row")
        let cell =
            outline.makeView(withIdentifier: identifier, owner: nil) as? ReceiptRowCellView
            ?? ReceiptRowCellView(identifier: identifier)
        cell.configure(row)
        return cell
    }

    func outlineView(_ outlineView: NSOutlineView, shouldSelectItem item: Any) -> Bool {
        false
    }
}

// One check/prepare row: state glyph, name, required badge, detail.
private final class ReceiptRowCellView: NSTableCellView {
    private let icon = NSImageView()
    private let spinner = NSProgressIndicator()
    private let name = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false

        name.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
        name.textColor = .labelColor
        name.lineBreakMode = .byTruncatingTail

        detail.font = .monospacedDigitSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .regular)
        detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingTail
        detail.setContentHuggingPriority(.required, for: .horizontal)

        for view in [icon, spinner, name, detail] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            icon.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
            spinner.centerXAnchor.constraint(equalTo: icon.centerXAnchor),
            spinner.centerYAnchor.constraint(equalTo: icon.centerYAnchor),
            name.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 5),
            name.centerYAnchor.constraint(equalTo: centerYAnchor),
            detail.leadingAnchor.constraint(
                greaterThanOrEqualTo: name.trailingAnchor, constant: Metrics.padding),
            detail.trailingAnchor.constraint(
                equalTo: trailingAnchor, constant: -Metrics.paddingWide),
            detail.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func configure(_ row: ReceiptRowItem) {
        let prefix = row.kind == .prepare ? "prepare: " : ""
        let suffix = row.kind == .check && !row.required && row.state != .pending ? "  (extra)" : ""
        name.stringValue = prefix + row.name + suffix

        if row.state == .running {
            icon.image = nil
            spinner.startAnimation(nil)
        } else {
            spinner.stopAnimation(nil)
            let (symbol, tint): (String, NSColor) = {
                switch row.state {
                case .pending: return ("circle", .tertiaryLabelColor)
                case .running: return ("circle", .tertiaryLabelColor)
                case .ok: return ("checkmark.circle.fill", .systemGreen)
                case .fail: return ("xmark.circle.fill", .systemRed)
                case .missing: return ("circle.dashed", .secondaryLabelColor)
                case .staleDefinition:
                    return ("exclamationmark.arrow.trianglehead.2.clockwise.rotate.90", .systemOrange)
                }
            }()
            icon.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            icon.contentTintColor = tint
        }

        switch row.state {
        case .missing: detail.stringValue = "no receipt for this tree"
        case .staleDefinition: detail.stringValue = "receipt predates the current check definition"
        case .running: detail.stringValue = "running\u{2026}"
        case .pending: detail.stringValue = ""
        default: detail.stringValue = row.detail
        }
    }
}

/// "8.1s" / "412ms" — matches the engine's own duration formatting spirit.
func formatDuration(_ ms: Double) -> String {
    if ms >= 60_000 {
        let minutes = Int(ms / 60_000)
        let seconds = Int((ms.truncatingRemainder(dividingBy: 60_000)) / 1000)
        return "\(minutes)m \(seconds)s"
    }
    if ms >= 1000 {
        return String(format: "%.1fs", ms / 1000)
    }
    return "\(Int(ms))ms"
}
