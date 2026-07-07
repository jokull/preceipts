// The thread area: the trailing pane IS the conversation. Default state
// shows the PR-level discussion; activating a line's thread (bubble
// double-click in the diff) breadcrumb-navigates into it; back pops.
// Reading, replying-as-draft-note, editing, and composing new drafts
// all happen here — the diff keeps only bubbles and the claw.

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

    var contexts: [CommentContext] {
        switch self {
        case .thread(let thread):
            return thread.comments.map(\.context)
        case .draft(let draft):
            return [
                CommentContext(
                    path: draft.path, line: draft.line, startLine: draft.startLine,
                    lineText: draft.quote ?? draft.lineText,
                    body: draft.body, author: nil)
            ]
        }
    }
}

final class ThreadAreaViewController: NSViewController {
    var onRefresh: (() -> Void)?

    private var content: TearContent?
    private var handlers = TearHandlers()

    private let backButton = NSButton()
    private let breadcrumb = NSTextField(labelWithString: "Feedback")
    private let bodyStack = NSStackView()
    private let scroll = NSScrollView()
    private let statusLabel = NSTextField(wrappingLabelWithString: "")
    private let composer = ComposerView()

    // ------------------------------------------------------------------
    // View construction

    override func loadView() {
        backButton.image = NSImage(
            systemSymbolName: "chevron.backward", accessibilityDescription: "back")
        backButton.isBordered = false
        backButton.controlSize = .small
        backButton.target = self
        backButton.action = #selector(backClicked(_:))
        backButton.toolTip = "Back to the PR conversation"
        backButton.isHidden = true

        breadcrumb.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        breadcrumb.lineBreakMode = .byTruncatingHead
        breadcrumb.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let copy = headerButton("doc.on.doc", "Copy as markdown", #selector(copyClicked(_:)))
        let open = headerButton("safari", "Open on GitHub", #selector(openClicked(_:)))
        let refresh = headerButton(
            "arrow.clockwise", "Fetch PR feedback", #selector(refreshClicked(_:)))

        let header = NSStackView(views: [
            backButton, breadcrumb, NSView(), copy, open, refresh,
        ])
        header.orientation = .horizontal
        header.spacing = Metrics.unit + 2
        header.edgeInsets = NSEdgeInsets(
            top: Metrics.padding, left: Metrics.paddingWide,
            bottom: 0, right: Metrics.padding)

        bodyStack.orientation = .vertical
        bodyStack.alignment = .leading
        bodyStack.spacing = Metrics.paddingWide

        let document = ThreadAreaDocument(stack: bodyStack)
        scroll.documentView = document
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false

        statusLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        statusLabel.textColor = .secondaryLabelColor

        composer.isHidden = true

        let stack = NSStackView(views: [header, scroll, statusLabel, composer])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = Metrics.padding
        stack.edgeInsets = NSEdgeInsets(
            top: 0, left: 0, bottom: Metrics.paddingWide, right: 0)
        scroll.setContentHuggingPriority(.init(1), for: .vertical)

        self.view = stack
        NSLayoutConstraint.activate([
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor),
            document.widthAnchor.constraint(equalTo: scroll.widthAnchor),
            statusLabel.leadingAnchor.constraint(
                equalTo: stack.leadingAnchor, constant: Metrics.paddingWide),
            statusLabel.widthAnchor.constraint(
                lessThanOrEqualTo: stack.widthAnchor, constant: -2 * Metrics.paddingWide),
            composer.leadingAnchor.constraint(
                equalTo: stack.leadingAnchor, constant: Metrics.paddingWide),
            composer.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingWide),
        ])

        composer.onSave = { [weak self] body in
            self?.handlers.saveNote?(body)
        }
    }

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

    // ------------------------------------------------------------------
    // Rendering

    func render(_ content: TearContent, handlers: TearHandlers) {
        self.content = content
        self.handlers = handlers
        statusLabel.isHidden = true

        backButton.isHidden = !content.showBack
        breadcrumb.stringValue = content.breadcrumb
        breadcrumb.toolTip = content.breadcrumb

        bodyStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for entry in content.entries {
            bodyStack.addArrangedSubview(entryBlock(entry))
        }
        for note in content.notes {
            bodyStack.addArrangedSubview(noteBlock(note))
        }
        if content.entries.isEmpty, content.notes.isEmpty, let empty = content.emptyText {
            let label = NSTextField(wrappingLabelWithString: empty)
            label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            label.textColor = .secondaryLabelColor
            bodyStack.addArrangedSubview(label)
            label.widthAnchor.constraint(equalTo: bodyStack.widthAnchor).isActive = true
        }

        if let placeholder = content.composerPlaceholder {
            composer.isHidden = false
            composer.reset(placeholder: placeholder)
            if content.focusComposer {
                DispatchQueue.main.async { [weak self] in
                    self?.composer.focus()
                }
            }
        } else {
            composer.isHidden = true
        }
        scroll.contentView.scroll(to: .zero)
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    /// Transient line under the header ("Fetching PR feedback…", errors).
    func showStatus(_ message: String) {
        statusLabel.stringValue = message
        statusLabel.isHidden = false
    }

    private func entryBlock(_ entry: TearContent.Entry) -> NSView {
        let block = NSStackView()
        block.orientation = .vertical
        block.alignment = .leading
        block.spacing = 2
        if let author = entry.author {
            let head = NSTextField(labelWithString: "\(author)  \(entry.meta)")
            head.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
            head.lineBreakMode = .byTruncatingTail
            block.addArrangedSubview(head)
        }
        let body = NSTextField(wrappingLabelWithString: entry.body)
        body.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
        body.isSelectable = true
        block.addArrangedSubview(body)
        body.widthAnchor.constraint(equalTo: block.widthAnchor).isActive = true
        return block
    }

    /// A draft note: always-editable, saved when focus leaves.
    private func noteBlock(_ note: LocalComment) -> NSView {
        let block = NSStackView()
        block.orientation = .vertical
        block.alignment = .leading
        block.spacing = 2

        let head = NSTextField(labelWithString: "Draft note")
        head.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        head.textColor = .controlAccentColor

        let delete = NSButton()
        delete.image = NSImage(
            systemSymbolName: "trash", accessibilityDescription: "delete note")
        delete.isBordered = false
        delete.controlSize = .small
        delete.target = self
        delete.action = #selector(deleteNoteClicked(_:))
        delete.tag = Int(bitPattern: UInt(note.id))
        delete.toolTip = "Delete this draft"

        let headRow = NSStackView(views: [head, delete])
        headRow.orientation = .horizontal
        headRow.spacing = Metrics.unit

        let editor = NoteEditor(note: note) { [weak self] id, body in
            self?.handlers.updateNote?(id, body)
        }

        block.addArrangedSubview(headRow)
        block.addArrangedSubview(editor)
        editor.widthAnchor.constraint(equalTo: block.widthAnchor).isActive = true
        return block
    }

    // ------------------------------------------------------------------
    // Actions

    @objc private func backClicked(_ sender: Any?) {
        handlers.back?()
    }

    @objc private func refreshClicked(_ sender: Any?) {
        onRefresh?()
    }

    @objc private func copyClicked(_ sender: Any?) {
        guard let content else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(content.digest, forType: .string)
    }

    @objc private func openClicked(_ sender: Any?) {
        guard let url = content?.url.flatMap(URL.init(string:)) else { return }
        NSWorkspace.shared.open(url)
    }

    @objc private func deleteNoteClicked(_ sender: NSButton) {
        handlers.deleteNote?(UInt64(UInt(bitPattern: sender.tag)))
    }

    override func cancelOperation(_ sender: Any?) {
        handlers.back?()
    }
}

/// Scroll document wrapping the body stack, top-anchored, padded.
private final class ThreadAreaDocument: NSView {
    override var isFlipped: Bool { true }

    init(stack: NSStackView) {
        super.init(frame: .zero)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor, constant: Metrics.unit),
            stack.leadingAnchor.constraint(
                equalTo: leadingAnchor, constant: Metrics.paddingWide),
            stack.trailingAnchor.constraint(
                equalTo: trailingAnchor, constant: -Metrics.paddingWide),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -Metrics.unit),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}
