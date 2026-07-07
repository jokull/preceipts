// The anchored thread card: a floating material bubble pinned beside the
// commented line range, scrolling with the diff. Long conversations
// scroll inside the card (capped height) — the diff never reflows.

import AppKit
import PreceiptsKit

struct ThreadCardModel {
    struct Entry {
        let author: String?
        let meta: String
        let body: String
    }

    let title: String
    let entries: [Entry]
    /// Root comment URL; nil for drafts.
    let url: String?
    let digest: String
}

final class ThreadCardView: NSVisualEffectView {
    static let width: CGFloat = 380
    static let maxBodyHeight: CGFloat = 300

    var onClose: (() -> Void)?

    private let titleLabel = NSTextField(labelWithString: "")
    private let entriesStack = NSStackView()
    private let bodyScroll = NSScrollView()
    private var model: ThreadCardModel?
    private var bodyHeight: NSLayoutConstraint?

    init() {
        super.init(frame: .zero)
        material = .popover
        blendingMode = .withinWindow
        state = .active
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.borderWidth = 1
        layer?.borderColor = NSColor.separatorColor.cgColor
        layer?.masksToBounds = true
        shadow = {
            let shadow = NSShadow()
            shadow.shadowBlurRadius = 12
            shadow.shadowOffset = NSSize(width: 0, height: -4)
            shadow.shadowColor = NSColor.black.withAlphaComponent(0.25)
            return shadow
        }()

        titleLabel.font = .monospacedSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        titleLabel.textColor = .secondaryLabelColor
        titleLabel.lineBreakMode = .byTruncatingHead

        let copy = headerButton("doc.on.doc", "Copy thread", #selector(copyClicked(_:)))
        let open = headerButton("safari", "Open on GitHub", #selector(openClicked(_:)))
        let close = headerButton("xmark", "Close (Esc)", #selector(closeClicked(_:)))

        let header = NSStackView(views: [titleLabel, NSView(), copy, open, close])
        header.orientation = .horizontal
        header.spacing = Metrics.unit

        entriesStack.orientation = .vertical
        entriesStack.alignment = .leading
        entriesStack.spacing = Metrics.paddingWide

        let clipDocument = FlippedStackDocument(stack: entriesStack)
        bodyScroll.documentView = clipDocument
        bodyScroll.hasVerticalScroller = true
        bodyScroll.drawsBackground = false
        bodyScroll.borderType = .noBorder

        let stack = NSStackView(views: [header, bodyScroll])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = Metrics.padding
        stack.edgeInsets = NSEdgeInsets(
            top: Metrics.paddingWide, left: Metrics.paddingWide,
            bottom: Metrics.paddingWide, right: Metrics.paddingWide)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)

        let bodyHeight = bodyScroll.heightAnchor.constraint(equalToConstant: 100)
        self.bodyHeight = bodyHeight
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            header.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingWide),
            bodyScroll.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingWide),
            bodyHeight,
            clipDocument.widthAnchor.constraint(
                equalTo: bodyScroll.widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func headerButton(
        _ symbol: String, _ tooltip: String, _ action: Selector
    ) -> NSButton {
        let button = NSButton()
        button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: tooltip)
        button.isBordered = false
        button.controlSize = .small
        button.target = self
        button.action = action
        button.toolTip = tooltip
        return button
    }

    /// Populate and return the fitting card height for layout.
    @discardableResult
    func show(_ model: ThreadCardModel) -> CGFloat {
        self.model = model
        titleLabel.stringValue = model.title
        entriesStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for entry in model.entries {
            let block = NSStackView()
            block.orientation = .vertical
            block.alignment = .leading
            block.spacing = 2
            if let author = entry.author {
                let head = NSTextField(labelWithString: "\(author)  \(entry.meta)")
                head.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
                head.textColor = .labelColor
                block.addArrangedSubview(head)
            }
            let body = NSTextField(wrappingLabelWithString: entry.body)
            body.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            body.textColor = .labelColor
            body.isSelectable = true
            block.addArrangedSubview(body)
            entriesStack.addArrangedSubview(block)
            NSLayoutConstraint.activate([
                block.widthAnchor.constraint(equalTo: entriesStack.widthAnchor),
                body.widthAnchor.constraint(equalTo: block.widthAnchor),
            ])
        }
        // Measure content against the fixed width, cap the scroll height.
        entriesStack.layoutSubtreeIfNeeded()
        let content = entriesStack.fittingSize.height
        let capped = min(content, Self.maxBodyHeight)
        bodyHeight?.constant = max(capped, 24)
        layoutSubtreeIfNeeded()
        return fittingSize.height
    }

    @objc private func copyClicked(_ sender: Any?) {
        guard let model else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(model.digest, forType: .string)
    }

    @objc private func openClicked(_ sender: Any?) {
        guard let model, let url = model.url.flatMap(URL.init(string:)) else { return }
        NSWorkspace.shared.open(url)
    }

    @objc private func closeClicked(_ sender: Any?) {
        onClose?()
    }
}

// ------------------------------------------------------------------
// Tear

/// A full-width conversation torn into the diff below its anchor range —
/// it reads in the main scroll flow (no inner scroll), like an inline PR
/// thread. Chrome regime inside the One Dark surface: the tear is the
/// app showing through the document.
final class ThreadTearView: NSView {
    var onClose: (() -> Void)?

    private let titleLabel = NSTextField(labelWithString: "")
    private let entriesStack = NSStackView()
    private let contentStack = NSStackView()
    private var model: ThreadCardModel?
    private var lastWidth: CGFloat = 0

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        titleLabel.font = .monospacedSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        titleLabel.textColor = .secondaryLabelColor
        titleLabel.lineBreakMode = .byTruncatingHead

        let copy = button("doc.on.doc", "Copy thread", #selector(copyClicked(_:)))
        let open = button("safari", "Open on GitHub", #selector(openClicked(_:)))
        let close = button("xmark", "Close (Esc)", #selector(closeClicked(_:)))

        let header = NSStackView(views: [titleLabel, NSView(), copy, open, close])
        header.orientation = .horizontal
        header.spacing = Metrics.unit

        entriesStack.orientation = .vertical
        entriesStack.alignment = .leading
        entriesStack.spacing = Metrics.paddingWide

        contentStack.orientation = .vertical
        contentStack.alignment = .leading
        contentStack.spacing = Metrics.padding
        contentStack.edgeInsets = NSEdgeInsets(
            top: Metrics.paddingWide, left: 2 * Metrics.paddingWide,
            bottom: Metrics.paddingWide, right: 2 * Metrics.paddingWide)
        contentStack.addArrangedSubview(header)
        contentStack.addArrangedSubview(entriesStack)

        let top = NSBox()
        top.boxType = .separator
        let bottom = NSBox()
        bottom.boxType = .separator

        for view in [contentStack, top, bottom] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            top.topAnchor.constraint(equalTo: topAnchor),
            top.leadingAnchor.constraint(equalTo: leadingAnchor),
            top.trailingAnchor.constraint(equalTo: trailingAnchor),
            bottom.bottomAnchor.constraint(equalTo: bottomAnchor),
            bottom.leadingAnchor.constraint(equalTo: leadingAnchor),
            bottom.trailingAnchor.constraint(equalTo: trailingAnchor),
            contentStack.topAnchor.constraint(equalTo: topAnchor),
            contentStack.leadingAnchor.constraint(equalTo: leadingAnchor),
            contentStack.trailingAnchor.constraint(equalTo: trailingAnchor),
            header.widthAnchor.constraint(
                equalTo: contentStack.widthAnchor, constant: -4 * Metrics.paddingWide),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private func button(
        _ symbol: String, _ tooltip: String, _ action: Selector
    ) -> NSButton {
        let button = NSButton()
        button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: tooltip)
        button.isBordered = false
        button.controlSize = .small
        button.target = self
        button.action = action
        button.toolTip = tooltip
        return button
    }

    /// Populate and measure for `width`; returns the tear row height.
    func prepare(_ model: ThreadCardModel, width: CGFloat) -> CGFloat {
        self.model = model
        titleLabel.stringValue = model.title
        entriesStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        // Comfortable read width even on wide diffs.
        let readable = min(width - 4 * Metrics.paddingWide, 680)
        for entry in model.entries {
            let block = NSStackView()
            block.orientation = .vertical
            block.alignment = .leading
            block.spacing = 2
            if let author = entry.author {
                let head = NSTextField(labelWithString: "\(author)  \(entry.meta)")
                head.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
                block.addArrangedSubview(head)
            }
            let body = NSTextField(wrappingLabelWithString: entry.body)
            body.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
            body.isSelectable = true
            body.preferredMaxLayoutWidth = readable
            block.addArrangedSubview(body)
            entriesStack.addArrangedSubview(block)
            body.widthAnchor.constraint(lessThanOrEqualToConstant: readable).isActive = true
        }
        return remeasure(width: width)
    }

    /// Height for the current model at `width` (wrapping text reflows).
    func remeasure(width: CGFloat) -> CGFloat {
        lastWidth = width
        frame = NSRect(x: 0, y: 0, width: width, height: 10)
        layoutSubtreeIfNeeded()
        return contentStack.fittingSize.height
    }

    @objc private func copyClicked(_ sender: Any?) {
        guard let model else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(model.digest, forType: .string)
    }

    @objc private func openClicked(_ sender: Any?) {
        guard let model, let url = model.url.flatMap(URL.init(string:)) else { return }
        NSWorkspace.shared.open(url)
    }

    @objc private func closeClicked(_ sender: Any?) {
        onClose?()
    }
}

/// Scroll document wrapping the entries stack, top-anchored.
private final class FlippedStackDocument: NSView {
    override var isFlipped: Bool { true }

    init(stack: NSStackView) {
        super.init(frame: .zero)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            // Document height follows content so the scroll view can scroll.
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}
