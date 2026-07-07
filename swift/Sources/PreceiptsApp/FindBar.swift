// ⌘F chrome: the find bar above the surface and the scrollbar-edge tick
// marks. Matching is Kit logic (FindMatcher); these views are display +
// input only.

import AppKit

final class FindBarView: NSVisualEffectView {
    var onQueryChange: ((String) -> Void)?
    var onStep: ((Int) -> Void)?
    var onClose: (() -> Void)?

    private let searchField = NSSearchField()
    private let count = NSTextField(labelWithString: "")
    private let stepper = NSSegmentedControl()

    var query: String { searchField.stringValue }
    var isOpen: Bool { !isHidden }

    init() {
        super.init(frame: .zero)
        material = .headerView
        blendingMode = .withinWindow

        searchField.placeholderString = "Find in diff"
        searchField.controlSize = .small
        searchField.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        searchField.delegate = self
        searchField.sendsWholeSearchString = false

        count.font = .monospacedDigitSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .regular)
        count.textColor = .secondaryLabelColor

        stepper.segmentCount = 2
        stepper.trackingMode = .momentary
        stepper.controlSize = .small
        stepper.setImage(
            NSImage(systemSymbolName: "chevron.up", accessibilityDescription: "previous match"),
            forSegment: 0)
        stepper.setImage(
            NSImage(systemSymbolName: "chevron.down", accessibilityDescription: "next match"),
            forSegment: 1)
        stepper.target = self
        stepper.action = #selector(stepClicked(_:))
        stepper.isEnabled = false

        let done = NSButton(title: "Done", target: self, action: #selector(doneClicked(_:)))
        done.bezelStyle = .accessoryBarAction
        done.controlSize = .small
        done.font = .systemFont(ofSize: NSFont.smallSystemFontSize)

        let spacer = NSView()
        spacer.setContentHuggingPriority(.init(1), for: .horizontal)

        let stack = NSStackView(views: [searchField, count, stepper, spacer, done])
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = Metrics.padding
        stack.edgeInsets = NSEdgeInsets(
            top: 0, left: Metrics.paddingWide, bottom: 0, right: Metrics.paddingWide)

        let separator = NSBox()
        separator.boxType = .separator

        for view in [stack, separator] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            searchField.widthAnchor.constraint(equalToConstant: 240),
            separator.leadingAnchor.constraint(equalTo: leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: trailingAnchor),
            separator.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func focus() {
        window?.makeFirstResponder(searchField)
        searchField.selectText(nil)
    }

    func clear() {
        searchField.stringValue = ""
        count.stringValue = ""
        stepper.isEnabled = false
    }

    func showCount(current: Int, total: Int, query: String) {
        if total == 0 {
            count.stringValue = query.isEmpty ? "" : "Not found"
        } else {
            count.stringValue = "\(current + 1) of \(total)"
        }
        stepper.isEnabled = total > 0
    }

    @objc private func stepClicked(_ sender: NSSegmentedControl) {
        onStep?(sender.selectedSegment == 0 ? -1 : 1)
    }

    @objc private func doneClicked(_ sender: Any?) {
        onClose?()
    }
}

extension FindBarView: NSSearchFieldDelegate {
    func controlTextDidChange(_ notification: Notification) {
        onQueryChange?(searchField.stringValue)
    }

    func control(
        _ control: NSControl, textView: NSTextView, doCommandBy selector: Selector
    ) -> Bool {
        switch selector {
        case #selector(NSResponder.cancelOperation(_:)):
            onClose?()
            return true
        case #selector(NSResponder.insertNewline(_:)):
            let shift = NSApp.currentEvent?.modifierFlags.contains(.shift) ?? false
            onStep?(shift ? -1 : 1)
            return true
        default:
            return false
        }
    }
}

/// Match positions along the scroll range, drawn at the scroller edge.
final class FindTicksView: NSView {
    private var fractions: [CGFloat] = []
    private var currentFraction: CGFloat?

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func update(matches: [Int], current: Int, totalRows: Int) {
        guard totalRows > 0 else {
            fractions = []
            currentFraction = nil
            needsDisplay = true
            return
        }
        fractions = matches.map { CGFloat($0) / CGFloat(totalRows) }
        currentFraction = current < matches.count
            ? CGFloat(matches[current]) / CGFloat(totalRows) : nil
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        for fraction in fractions {
            let y = fraction * bounds.height
            NSColor.findHighlightColor.withAlphaComponent(0.8).setFill()
            NSRect(x: 1, y: y, width: bounds.width - 4, height: 2).fill()
        }
        if let currentFraction {
            NSColor.controlAccentColor.setFill()
            NSRect(x: 0, y: currentFraction * bounds.height - 1, width: bounds.width, height: 3)
                .fill()
        }
    }
}
