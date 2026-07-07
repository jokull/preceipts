// The thread area: a full-width tear in the diff. Its empty state is the
// PR conversation (torn in above the first file); selecting an anchored
// thread moves the tear to that range with a breadcrumb + back button.
// The kitchen sink lives here: read the thread, edit/delete your draft
// notes, and compose new ones — never posted, notes-to-agent.

import AppKit
import PreceiptsKit

struct TearContent {
    struct Entry {
        let author: String?
        let meta: String
        let body: String
    }

    /// "‹" target exists when non-nil handler is installed; label text.
    var breadcrumb: String
    var showBack: Bool
    /// Read-only comments (GitHub).
    var entries: [Entry]
    /// Editable local draft notes.
    var notes: [LocalComment]
    var url: String?
    var digest: String
    /// Nil hides the composer (conversation view).
    var composerPlaceholder: String?
    var focusComposer: Bool
    /// Shown when there's nothing else (empty conversation, no PR…).
    var emptyText: String?
}

struct TearHandlers {
    var back: (() -> Void)?
    var saveNote: ((String) -> Void)?
    var updateNote: ((UInt64, String) -> Void)?
    var deleteNote: ((UInt64) -> Void)?
}

final class ThreadTearView: NSView {
    var onClose: (() -> Void)?

    private var content: TearContent?
    private var handlers = TearHandlers()

    private let backButton = NSButton()
    private let breadcrumbLabel = NSTextField(labelWithString: "")
    private let bodyStack = NSStackView()
    private let composer: ComposerView
    private let contentStack = NSStackView()

    init() {
        composer = ComposerView()
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        backButton.image = NSImage(
            systemSymbolName: "chevron.backward", accessibilityDescription: "back")
        backButton.isBordered = false
        backButton.controlSize = .small
        backButton.target = self
        backButton.action = #selector(backClicked(_:))
        backButton.toolTip = "Back to the PR conversation"

        breadcrumbLabel.font = .monospacedSystemFont(
            ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        breadcrumbLabel.textColor = .secondaryLabelColor
        breadcrumbLabel.lineBreakMode = .byTruncatingHead

        let copy = button("doc.on.doc", "Copy thread", #selector(copyClicked(_:)))
        let open = button("safari", "Open on GitHub", #selector(openClicked(_:)))
        let close = button("xmark", "Close (Esc)", #selector(closeClicked(_:)))

        let header = NSStackView(views: [
            backButton, breadcrumbLabel, NSView(), copy, open, close,
        ])
        header.orientation = .horizontal
        header.spacing = Metrics.unit + 2

        bodyStack.orientation = .vertical
        bodyStack.alignment = .leading
        bodyStack.spacing = Metrics.paddingWide

        composer.onSave = { [weak self] body in
            self?.handlers.saveNote?(body)
        }

        contentStack.orientation = .vertical
        contentStack.alignment = .leading
        contentStack.spacing = Metrics.paddingWide
        contentStack.edgeInsets = NSEdgeInsets(
            top: Metrics.paddingWide, left: 2 * Metrics.paddingWide,
            bottom: Metrics.paddingWide, right: 2 * Metrics.paddingWide)
        contentStack.addArrangedSubview(header)
        contentStack.addArrangedSubview(bodyStack)
        contentStack.addArrangedSubview(composer)

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

    // ------------------------------------------------------------------
    // Population + measurement

    /// Populate and measure for `width`; returns the tear row height.
    func prepare(_ content: TearContent, handlers: TearHandlers, width: CGFloat) -> CGFloat {
        self.content = content
        self.handlers = handlers

        backButton.isHidden = !content.showBack
        breadcrumbLabel.stringValue = content.breadcrumb

        bodyStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        let readable = min(width - 4 * Metrics.paddingWide, 680)

        for entry in content.entries {
            bodyStack.addArrangedSubview(entryBlock(entry, readable: readable))
        }
        for note in content.notes {
            bodyStack.addArrangedSubview(noteBlock(note, readable: readable))
        }
        if content.entries.isEmpty, content.notes.isEmpty, let empty = content.emptyText {
            let label = NSTextField(wrappingLabelWithString: empty)
            label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            label.textColor = .secondaryLabelColor
            label.preferredMaxLayoutWidth = readable
            bodyStack.addArrangedSubview(label)
        }

        if let placeholder = content.composerPlaceholder {
            composer.isHidden = false
            composer.reset(placeholder: placeholder, width: readable)
        } else {
            composer.isHidden = true
        }

        return remeasure(width: width)
    }

    /// Height for the current content at `width` (wrapping text reflows).
    func remeasure(width: CGFloat) -> CGFloat {
        frame = NSRect(x: 0, y: 0, width: width, height: 10)
        layoutSubtreeIfNeeded()
        return contentStack.fittingSize.height
    }

    func focusComposerIfRequested() {
        if content?.focusComposer == true, !composer.isHidden {
            composer.focus()
        }
    }

    private func entryBlock(_ entry: TearContent.Entry, readable: CGFloat) -> NSView {
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
        body.widthAnchor.constraint(lessThanOrEqualToConstant: readable).isActive = true
        block.addArrangedSubview(body)
        return block
    }

    /// A draft note: always-editable text, saved when focus leaves.
    private func noteBlock(_ note: LocalComment, readable: CGFloat) -> NSView {
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
        editor.widthAnchor.constraint(equalToConstant: readable).isActive = true

        block.addArrangedSubview(headRow)
        block.addArrangedSubview(editor)
        return block
    }

    // ------------------------------------------------------------------
    // Actions

    @objc private func backClicked(_ sender: Any?) {
        handlers.back?()
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

    @objc private func closeClicked(_ sender: Any?) {
        onClose?()
    }

    @objc private func deleteNoteClicked(_ sender: NSButton) {
        handlers.deleteNote?(UInt64(UInt(bitPattern: sender.tag)))
    }
}

// ------------------------------------------------------------------
// Note editor: bordered NSTextView, commits on focus loss.

private final class NoteEditor: NSView, NSTextViewDelegate {
    private let noteId: UInt64
    private let onCommit: (UInt64, String) -> Void
    private let textView: NSTextView
    private let scroll: NSScrollView

    init(note: LocalComment, onCommit: @escaping (UInt64, String) -> Void) {
        self.noteId = note.id
        self.onCommit = onCommit
        self.scroll = NSTextView.scrollableTextView()
        self.textView = scroll.documentView as! NSTextView
        super.init(frame: .zero)

        textView.string = note.body
        textView.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
        textView.isRichText = false
        textView.textContainerInset = NSSize(width: 4, height: 4)
        textView.delegate = self
        scroll.borderType = .bezelBorder
        scroll.hasVerticalScroller = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)

        // Size to content (within reason) — the tear grows, not the note.
        let measured = note.body.height(
            font: textView.font!, width: 640) + 14
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            heightAnchor.constraint(equalToConstant: min(max(measured, 28), 200)),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func textDidEndEditing(_ notification: Notification) {
        onCommit(noteId, textView.string)
    }
}

// ------------------------------------------------------------------
// Composer: text view + save, for new notes and thread replies.

private final class ComposerView: NSStackView {
    var onSave: ((String) -> Void)?

    private let textView: NSTextView
    private let textScroll: NSScrollView
    private let saveButton: NSButton

    init() {
        textScroll = NSTextView.scrollableTextView()
        textView = textScroll.documentView as! NSTextView
        saveButton = NSButton(title: "Save Note", target: nil, action: nil)
        super.init(frame: .zero)

        textView.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
        textView.isRichText = false
        textView.textContainerInset = NSSize(width: 4, height: 5)
        textScroll.borderType = .bezelBorder
        textScroll.hasVerticalScroller = false

        saveButton.bezelStyle = .rounded
        saveButton.controlSize = .small
        saveButton.keyEquivalent = "\r"
        saveButton.keyEquivalentModifierMask = [.command]
        saveButton.target = self
        saveButton.action = #selector(saveClicked(_:))

        let hint = NSTextField(labelWithString: "Never posted \u{2014} a note for your agent")
        hint.font = .systemFont(ofSize: NSFont.smallSystemFontSize - 1)
        hint.textColor = .secondaryLabelColor

        let footer = NSStackView(views: [hint, NSView(), saveButton])
        footer.orientation = .horizontal
        footer.spacing = Metrics.padding

        orientation = .vertical
        alignment = .leading
        spacing = Metrics.unit + 2
        addArrangedSubview(textScroll)
        addArrangedSubview(footer)
        NSLayoutConstraint.activate([
            textScroll.heightAnchor.constraint(equalToConstant: 64),
            footer.widthAnchor.constraint(equalTo: widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func reset(placeholder: String, width: CGFloat) {
        textView.string = ""
        // NSTextView has no placeholder; the hint label carries intent.
        toolTip = placeholder
        textScroll.widthAnchor.constraint(equalToConstant: width).isActive = true
    }

    func focus() {
        window?.makeFirstResponder(textView)
    }

    @objc private func saveClicked(_ sender: Any?) {
        let body = textView.string
        guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        onSave?(body)
    }
}

extension String {
    fileprivate func height(font: NSFont, width: CGFloat) -> CGFloat {
        let bounds = (self as NSString).boundingRect(
            with: NSSize(width: width, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin],
            attributes: [.font: font])
        return ceil(bounds.height)
    }
}
