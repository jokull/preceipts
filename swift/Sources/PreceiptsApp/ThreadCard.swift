// Thread-area building blocks: the content model the cockpit renders
// into the trailing pane, plus the editable note and composer views.
// Never posted — drafts are notes-to-agent.

import AppKit
import PreceiptsKit

struct TearContent {
    struct Entry {
        let author: String?
        let meta: String
        let body: String
    }

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

// ------------------------------------------------------------------
// Note editor: bordered NSTextView, commits on focus loss.

final class NoteEditor: NSView, NSTextViewDelegate {
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

        // Size to content (within reason) — the pane scrolls, not the note.
        let measured = note.body.height(font: textView.font!, width: 300) + 14
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            heightAnchor.constraint(equalToConstant: min(max(measured, 28), 220)),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func textDidEndEditing(_ notification: Notification) {
        onCommit(noteId, textView.string)
    }
}

// ------------------------------------------------------------------
// Composer: text view + save, for new notes and thread replies.

final class ComposerView: NSStackView {
    var onSave: ((String) -> Void)?

    private let textView: NSTextView
    private let textScroll: NSScrollView
    private let saveButton: NSButton
    private let hint = NSTextField(
        labelWithString: "Never posted \u{2014} a note for your agent")

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

        hint.font = .systemFont(ofSize: NSFont.smallSystemFontSize - 1)
        hint.textColor = .secondaryLabelColor
        hint.lineBreakMode = .byTruncatingTail

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
            textScroll.widthAnchor.constraint(equalTo: widthAnchor),
            footer.widthAnchor.constraint(equalTo: widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func reset(placeholder: String) {
        textView.string = ""
        // NSTextView has no placeholder; the hint label carries intent.
        hint.stringValue = "\(placeholder) \u{2014} never posted"
        toolTip = placeholder
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
    func height(font: NSFont, width: CGFloat) -> CGFloat {
        let bounds = (self as NSString).boundingRect(
            with: NSSize(width: width, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin],
            attributes: [.font: font])
        return ceil(bounds.height)
    }
}
