// The thread area: the trailing pane IS the conversation. Default state
// shows the PR-level discussion; activating a line's thread (bubble
// click in the diff) breadcrumb-navigates into it; back pops.
// Reading, replying-as-draft-note, editing your own comments, resolving
// the thread, and composing new drafts all happen here — the diff keeps
// only bubbles and the claw.

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
    private let resolveBar = NSStackView()
    private let resolveButton = NSButton()
    private let composer = ComposerView()

    // ------------------------------------------------------------------
    // View construction

    override func loadView() {
        // Header controls at HIG size — this pane is chrome, not a
        // technical cockpit; ≥24pt hit targets, breathing room at the
        // pane's rounded corners.
        backButton.image = NSImage(
            systemSymbolName: "chevron.backward", accessibilityDescription: "back")?
            .withSymbolConfiguration(.init(pointSize: 13, weight: .semibold))
        backButton.isBordered = false
        backButton.imagePosition = .imageOnly
        backButton.target = self
        backButton.action = #selector(backClicked(_:))
        backButton.toolTip = "Back to the PR conversation"
        backButton.isHidden = true
        backButton.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            backButton.widthAnchor.constraint(equalToConstant: 26),
            backButton.heightAnchor.constraint(equalToConstant: 26),
        ])

        breadcrumb.font = .systemFont(ofSize: 13, weight: .semibold)
        breadcrumb.lineBreakMode = .byTruncatingHead
        breadcrumb.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let copy = ThreadStyle.iconButton(
            "doc.on.doc", tooltip: "Copy thread as markdown",
            target: self, action: #selector(copyClicked(_:)))
        let open = ThreadStyle.iconButton(
            "safari", tooltip: "Open on GitHub",
            target: self, action: #selector(openClicked(_:)))
        let refresh = ThreadStyle.iconButton(
            "arrow.clockwise", tooltip: "Fetch PR feedback",
            target: self, action: #selector(refreshClicked(_:)))

        let header = NSStackView(views: [
            backButton, breadcrumb, NSView(), copy, open, refresh,
        ])
        header.orientation = .horizontal
        header.alignment = .centerY
        header.spacing = Metrics.unit
        header.setCustomSpacing(Metrics.padding, after: backButton)
        header.edgeInsets = NSEdgeInsets(
            top: Metrics.paddingWide, left: Metrics.paddingXL,
            bottom: Metrics.unit, right: Metrics.paddingXL)

        bodyStack.orientation = .vertical
        bodyStack.alignment = .leading
        bodyStack.spacing = Metrics.paddingWide

        let document = ThreadAreaDocument(stack: bodyStack)
        document.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = document
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        // Autolayout document: pin to the clip view, height from content.
        NSLayoutConstraint.activate([
            document.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            document.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])

        statusLabel.font = .systemFont(ofSize: 12)
        statusLabel.textColor = .secondaryLabelColor

        let headerRule = NSBox()
        headerRule.boxType = .separator

        resolveButton.bezelStyle = .rounded
        resolveButton.controlSize = .regular
        resolveButton.font = .systemFont(ofSize: 13)
        resolveButton.target = self
        resolveButton.action = #selector(resolveClicked(_:))
        resolveBar.orientation = .horizontal
        resolveBar.spacing = Metrics.padding
        resolveBar.addArrangedSubview(NSView())
        resolveBar.addArrangedSubview(resolveButton)
        resolveBar.isHidden = true

        composer.isHidden = true

        let stack = NSStackView(views: [
            header, headerRule, scroll, statusLabel, resolveBar, composer,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = Metrics.padding
        stack.setCustomSpacing(0, after: header)
        stack.edgeInsets = NSEdgeInsets(
            top: 0, left: 0, bottom: Metrics.paddingXL, right: 0)
        scroll.setContentHuggingPriority(.init(1), for: .vertical)

        self.view = stack
        NSLayoutConstraint.activate([
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            headerRule.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor),
            statusLabel.leadingAnchor.constraint(
                equalTo: stack.leadingAnchor, constant: Metrics.paddingXL),
            statusLabel.widthAnchor.constraint(
                lessThanOrEqualTo: stack.widthAnchor, constant: -2 * Metrics.paddingXL),
            resolveBar.leadingAnchor.constraint(
                equalTo: stack.leadingAnchor, constant: Metrics.paddingXL),
            resolveBar.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingXL),
            composer.leadingAnchor.constraint(
                equalTo: stack.leadingAnchor, constant: Metrics.paddingXL),
            composer.widthAnchor.constraint(
                equalTo: stack.widthAnchor, constant: -2 * Metrics.paddingXL),
        ])

        composer.onSave = { [weak self] body in
            self?.handlers.saveNote?(body)
        }
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

        if content.quote != nil || content.isOutdated || content.isResolved == true {
            let strip = quoteStrip(content)
            bodyStack.addArrangedSubview(strip)
            strip.widthAnchor.constraint(equalTo: bodyStack.widthAnchor).isActive = true
        }

        var first = true
        for entry in content.entries {
            if !first {
                addSeparator()
            }
            first = false
            let card = CommentCardView(entry: entry, onEdit: handlers.editComment)
            bodyStack.addArrangedSubview(card)
            card.widthAnchor.constraint(equalTo: bodyStack.widthAnchor).isActive = true
        }
        for note in content.notes {
            if !first {
                addSeparator()
            }
            first = false
            let block = noteBlock(note)
            bodyStack.addArrangedSubview(block)
            block.widthAnchor.constraint(equalTo: bodyStack.widthAnchor).isActive = true
        }
        if content.entries.isEmpty, content.notes.isEmpty, let empty = content.emptyText {
            bodyStack.addArrangedSubview(emptyState(empty))
        }

        // Resolution footer: GitHub muscle memory — the act lives at the
        // bottom of the thread, next to where you'd reply.
        if let isResolved = content.isResolved, handlers.setResolved != nil {
            resolveBar.isHidden = false
            resolveButton.title = isResolved ? "Unresolve" : "Resolve conversation"
            resolveButton.isEnabled = true
        } else {
            resolveBar.isHidden = true
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

    private func addSeparator() {
        let line = NSBox()
        line.boxType = .separator
        bodyStack.addArrangedSubview(line)
        line.widthAnchor.constraint(equalTo: bodyStack.widthAnchor).isActive = true
    }

    /// Anchored-code context strip: the quoted line(s) plus lifecycle
    /// chips (Resolved / outdated) — what the thread is *about*.
    private func quoteStrip(_ content: TearContent) -> NSView {
        let strip = NSStackView()
        strip.orientation = .vertical
        strip.alignment = .leading
        strip.spacing = Metrics.unit + 2

        var chips: [NSView] = []
        if content.isResolved == true {
            chips.append(ChipView("Resolved", color: .systemGreen))
        }
        if content.isOutdated {
            chips.append(ChipView("outdated", color: .systemOrange))
        }
        if !chips.isEmpty {
            let chipRow = NSStackView(views: chips + [NSView()])
            chipRow.orientation = .horizontal
            chipRow.spacing = Metrics.unit + 2
            strip.addArrangedSubview(chipRow)
        }

        if let quote = content.quote, !quote.isEmpty {
            let box = NSView()
            box.wantsLayer = true
            box.layer?.backgroundColor = NSColor.quaternarySystemFill.cgColor
            box.layer?.cornerRadius = 6

            let lines = quote.split(separator: "\n", omittingEmptySubsequences: false)
            let shown = lines.prefix(4).joined(separator: "\n")
                + (lines.count > 4 ? "\n\u{2026}" : "")
            let label = NSTextField(wrappingLabelWithString: shown)
            label.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
            label.textColor = .secondaryLabelColor
            label.translatesAutoresizingMaskIntoConstraints = false
            box.addSubview(label)
            NSLayoutConstraint.activate([
                label.topAnchor.constraint(equalTo: box.topAnchor, constant: 6),
                label.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -6),
                label.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 8),
                label.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -8),
            ])
            strip.addArrangedSubview(box)
            box.widthAnchor.constraint(equalTo: strip.widthAnchor).isActive = true
        }
        return strip
    }

    private func emptyState(_ text: String) -> NSView {
        let icon = NSImageView(
            image: NSImage(
                systemSymbolName: "bubble.left.and.bubble.right",
                accessibilityDescription: nil) ?? NSImage())
        icon.symbolConfiguration = .init(pointSize: 24, weight: .light)
        icon.contentTintColor = .tertiaryLabelColor
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = ThreadStyle.bodyFont
        label.textColor = .secondaryLabelColor
        label.alignment = .center
        let stack = NSStackView(views: [icon, label])
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = Metrics.padding
        stack.edgeInsets = NSEdgeInsets(top: 32, left: 0, bottom: 0, right: 0)
        bodyStack.addArrangedSubview(stack)
        stack.widthAnchor.constraint(equalTo: bodyStack.widthAnchor).isActive = true
        return stack
    }

    /// A draft note: always-editable, saved when focus leaves. Styled as
    /// a card whose "avatar" is the draft glyph, so GitHub comments and
    /// your notes read as one conversation.
    private func noteBlock(_ note: LocalComment) -> NSView {
        let glyph = NSImageView(
            image: NSImage(
                systemSymbolName: "square.and.pencil", accessibilityDescription: "draft")
                ?? NSImage())
        glyph.symbolConfiguration = .init(pointSize: 13, weight: .medium)
        glyph.contentTintColor = .controlAccentColor
        glyph.translatesAutoresizingMaskIntoConstraints = false
        glyph.widthAnchor.constraint(equalToConstant: ThreadStyle.avatarSize).isActive = true

        let head = NSTextField(labelWithString: "Draft note")
        head.font = ThreadStyle.authorFont
        head.textColor = .controlAccentColor

        let time = NSTextField(
            labelWithString: Self.relativeTime(unixSeconds: note.createdAt))
        time.font = ThreadStyle.metaFont
        time.textColor = .secondaryLabelColor

        let copy = ThreadStyle.iconButton(
            "doc.on.doc", tooltip: "Copy with context",
            target: self, action: #selector(copyNoteClicked(_:)),
            pointSize: 11, hitTarget: 22)
        copy.tag = Int(bitPattern: UInt(note.id))
        let delete = ThreadStyle.iconButton(
            "trash", tooltip: "Delete this draft",
            target: self, action: #selector(deleteNoteClicked(_:)),
            pointSize: 11, hitTarget: 22)
        delete.tag = Int(bitPattern: UInt(note.id))

        let headRow = NSStackView(views: [head, time, NSView(), copy, delete])
        headRow.orientation = .horizontal
        headRow.alignment = .centerY
        headRow.spacing = 6

        let editor = NoteEditor(note: note) { [weak self] id, body in
            self?.handlers.updateNote?(id, body)
        }

        let column = NSStackView(views: [headRow, editor])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 3

        let row = NSStackView(views: [glyph, column])
        row.orientation = .horizontal
        row.alignment = .top
        row.spacing = ThreadStyle.cardGap
        NSLayoutConstraint.activate([
            headRow.widthAnchor.constraint(equalTo: column.widthAnchor),
            editor.widthAnchor.constraint(equalTo: column.widthAnchor),
        ])
        return row
    }

    private static func relativeTime(unixSeconds: UInt64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(unixSeconds))
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
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

    @objc private func resolveClicked(_ sender: Any?) {
        guard let isResolved = content?.isResolved else { return }
        resolveButton.isEnabled = false
        handlers.setResolved?(!isResolved)
    }

    @objc private func copyNoteClicked(_ sender: NSButton) {
        let id = UInt64(UInt(bitPattern: sender.tag))
        guard let note = content?.notes.first(where: { $0.id == id }) else { return }
        let context = CommentContext(
            path: note.path, line: note.line, startLine: note.startLine,
            lineText: note.quote ?? note.lineText, body: note.body, author: nil)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(formatComment(context), forType: .string)
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
            stack.topAnchor.constraint(equalTo: topAnchor, constant: Metrics.padding),
            stack.leadingAnchor.constraint(
                equalTo: leadingAnchor, constant: Metrics.paddingXL),
            stack.trailingAnchor.constraint(
                equalTo: trailingAnchor, constant: -Metrics.paddingXL),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -Metrics.padding),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}
