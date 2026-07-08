// Thread-area building blocks: the content model the cockpit renders
// into the trailing pane, the comment card (avatar + header + body +
// hover actions + inline edit), and the editable note and composer
// views. Draft notes are never posted — they're notes-to-agent; edits
// to your own GitHub comments and thread resolution ARE posted (triage
// acts, per the design doc).

import AppKit
import PreceiptsKit

struct TearContent {
    struct Entry {
        let commentId: Int
        let kind: FeedbackKind
        let author: String
        let isBot: Bool
        let avatarUrl: String?
        let timeText: String
        /// Review verdict ("approved", "changes requested") — chip.
        let verdict: String?
        let body: String
        let url: String?
        /// The viewer authored this comment and the transport can PATCH it.
        let canEdit: Bool
        /// Markdown context block for the per-comment copy affordance.
        let copyText: String
    }

    var breadcrumb: String
    var showBack: Bool
    /// Anchored code context (thread anchor line / draft selection block).
    var quote: String?
    /// Thread resolution; nil when the view isn't a resolvable thread.
    var isResolved: Bool?
    var isOutdated: Bool
    /// Read-only comments (GitHub).
    var entries: [Entry]
    /// Editable local draft notes.
    var notes: [LocalComment]
    var url: String?
    var digest: String
    /// Nil hides the composer.
    var composerPlaceholder: String?
    var focusComposer: Bool
    /// Shown when there's nothing else (empty conversation, no PR…).
    var emptyText: String?

    init(
        breadcrumb: String, showBack: Bool, quote: String? = nil,
        isResolved: Bool? = nil, isOutdated: Bool = false,
        entries: [Entry], notes: [LocalComment], url: String?, digest: String,
        composerPlaceholder: String?, focusComposer: Bool, emptyText: String?
    ) {
        self.breadcrumb = breadcrumb
        self.showBack = showBack
        self.quote = quote
        self.isResolved = isResolved
        self.isOutdated = isOutdated
        self.entries = entries
        self.notes = notes
        self.url = url
        self.digest = digest
        self.composerPlaceholder = composerPlaceholder
        self.focusComposer = focusComposer
        self.emptyText = emptyText
    }
}

struct TearHandlers {
    var back: (() -> Void)?
    var saveNote: ((String) -> Void)?
    var updateNote: ((UInt64, String) -> Void)?
    var deleteNote: ((UInt64) -> Void)?
    /// Resolve/unresolve the thread on GitHub.
    var setResolved: ((Bool) -> Void)?
    /// PATCH the viewer's own comment body on GitHub.
    var editComment: ((TearContent.Entry, String) -> Void)?
}

/// Pane-wide layout + type tokens. Body text is standard macOS 13pt;
/// metadata drops to 11; everything sits on the 4pt grid. Controls are
/// HIG-sized — borderless icon buttons still get a ≥24pt hit target.
enum ThreadStyle {
    static let bodyFont = NSFont.systemFont(ofSize: 13)
    static let authorFont = NSFont.systemFont(ofSize: 13, weight: .semibold)
    static let metaFont = NSFont.systemFont(ofSize: 11)
    static let chipFont = NSFont.systemFont(ofSize: 11, weight: .medium)
    static let avatarSize: CGFloat = 24
    static let cardGap: CGFloat = 8

    /// Borderless SF Symbol button with a real (HIG) hit target.
    static func iconButton(
        _ symbol: String, tooltip: String, target: AnyObject?, action: Selector,
        pointSize: CGFloat = 13, hitTarget: CGFloat = 26
    ) -> NSButton {
        let button = NSButton()
        button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: tooltip)?
            .withSymbolConfiguration(.init(pointSize: pointSize, weight: .medium))
        button.isBordered = false
        button.imagePosition = .imageOnly
        button.target = target
        button.action = action
        button.toolTip = tooltip
        button.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            button.widthAnchor.constraint(equalToConstant: hitTarget),
            button.heightAnchor.constraint(equalToConstant: hitTarget),
        ])
        return button
    }
}

// ------------------------------------------------------------------
// Chip: tiny tinted capsule label ("bot", "approved", "Resolved"…).

final class ChipView: NSView {
    private let label: NSTextField

    init(_ text: String, color: NSColor) {
        label = NSTextField(labelWithString: text)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = color.withAlphaComponent(0.16).cgColor
        layer?.cornerRadius = 4
        label.font = ThreadStyle.chipFont
        label.textColor = color
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.topAnchor.constraint(equalTo: topAnchor, constant: 1),
            label.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -1),
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 5),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -5),
        ])
        setContentHuggingPriority(.required, for: .horizontal)
        setContentCompressionResistancePriority(.required, for: .horizontal)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}

// ------------------------------------------------------------------
// Comment card: one GitHub comment. Body renders as segments — text
// interleaved with native image views (GIFs animate). Actions are
// plain-text buttons in a footer row: Copy (flips to "Copied!"),
// Visit GitHub, Edit for the viewer's own comments.

final class CommentCardView: NSView {
    private let entry: TearContent.Entry
    private let onEdit: ((TearContent.Entry, String) -> Void)?

    private let contentColumn = NSStackView()
    private let bodyStack = NSStackView()
    private var segments: [BodySegment]
    private var expanded = false
    private var expandButton: NSButton?
    private var copyButton: NSButton?
    private var editBox: NSView?
    private var editTextView: NSTextView?

    /// Bodies longer than this collapse behind "Show more" — bot walls
    /// of text must not bury the conversation.
    private static let collapseThreshold = 18
    private static let collapsedLines = 10

    init(entry: TearContent.Entry, onEdit: ((TearContent.Entry, String) -> Void)?) {
        self.entry = entry
        self.onEdit = onEdit
        self.segments = MarkdownBody.render(entry.body)
        super.init(frame: .zero)

        let avatar = AvatarView(diameter: ThreadStyle.avatarSize)
        avatar.load(login: entry.author, urlString: entry.avatarUrl)
        avatar.setContentHuggingPriority(.required, for: .horizontal)

        let author = NSTextField(labelWithString: entry.author)
        author.font = ThreadStyle.authorFont
        author.lineBreakMode = .byTruncatingTail

        let time = NSTextField(labelWithString: entry.timeText)
        time.font = ThreadStyle.metaFont
        time.textColor = .secondaryLabelColor

        let header = NSStackView()
        header.orientation = .horizontal
        header.alignment = .centerY
        header.spacing = 6
        header.addArrangedSubview(author)
        if entry.isBot {
            header.addArrangedSubview(ChipView("bot", color: .secondaryLabelColor))
        }
        if let verdict = entry.verdict {
            header.addArrangedSubview(ChipView(verdict, color: Self.verdictColor(verdict)))
        }
        header.addArrangedSubview(time)
        header.addArrangedSubview(NSView())

        bodyStack.orientation = .vertical
        bodyStack.alignment = .leading
        bodyStack.spacing = Metrics.padding

        contentColumn.orientation = .vertical
        contentColumn.alignment = .leading
        contentColumn.spacing = Metrics.unit
        contentColumn.addArrangedSubview(header)
        contentColumn.addArrangedSubview(bodyStack)

        if segments.reduce(0, { $0 + $1.lineWeight }) > Self.collapseThreshold {
            let button = NSButton(
                title: "Show more", target: self, action: #selector(toggleExpanded))
            button.isBordered = false
            button.font = ThreadStyle.metaFont
            button.contentTintColor = .controlAccentColor
            contentColumn.addArrangedSubview(button)
            expandButton = button
        }
        populateBody()

        // Footer: plain-text actions, quiet but always present.
        let copy = Self.linkButton("Copy", target: self, action: #selector(copyClicked))
        copyButton = copy
        var footerButtons = [copy]
        if entry.url != nil {
            footerButtons.append(
                Self.linkButton(
                    "Visit GitHub", target: self, action: #selector(openClicked)))
        }
        if entry.canEdit, onEdit != nil {
            footerButtons.append(
                Self.linkButton("Edit", target: self, action: #selector(editClicked)))
        }
        let footer = NSStackView(views: footerButtons)
        footer.orientation = .horizontal
        footer.spacing = Metrics.paddingWide
        contentColumn.addArrangedSubview(footer)
        contentColumn.setCustomSpacing(Metrics.unit + 2, after: bodyStack)

        let row = NSStackView(views: [avatar, contentColumn])
        row.orientation = .horizontal
        row.alignment = .top
        row.spacing = ThreadStyle.cardGap
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: topAnchor),
            row.leadingAnchor.constraint(equalTo: leadingAnchor),
            row.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.bottomAnchor.constraint(equalTo: bottomAnchor),
            header.widthAnchor.constraint(equalTo: contentColumn.widthAnchor),
            bodyStack.widthAnchor.constraint(equalTo: contentColumn.widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    private static func linkButton(
        _ title: String, target: AnyObject, action: Selector
    ) -> NSButton {
        let button = NSButton(title: "", target: target, action: action)
        button.isBordered = false
        button.setContentHuggingPriority(.required, for: .horizontal)
        setLinkTitle(button, title, color: .secondaryLabelColor)
        return button
    }

    private static func setLinkTitle(_ button: NSButton, _ title: String, color: NSColor) {
        button.attributedTitle = NSAttributedString(
            string: title,
            attributes: [
                .font: NSFont.systemFont(ofSize: 12),
                .foregroundColor: color,
            ])
    }

    // ------------------------------------------------------------------
    // Body segments + collapse

    private func populateBody() {
        bodyStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        var budget = (expanded || expandButton == nil) ? Int.max : Self.collapsedLines
        for segment in segments {
            guard budget > 0 else { break }
            switch segment {
            case .text(let text):
                let weight = segment.lineWeight
                let display = weight > budget ? Self.truncate(text, toLines: budget) : text
                budget -= weight
                let label = Self.bodyTextField(display)
                bodyStack.addArrangedSubview(label)
                label.widthAnchor.constraint(equalTo: bodyStack.widthAnchor).isActive = true
            case .image(let url, let alt):
                budget -= segment.lineWeight
                let view = BodyImageView(url: url, alt: alt)
                bodyStack.addArrangedSubview(view)
                view.widthAnchor.constraint(
                    lessThanOrEqualTo: bodyStack.widthAnchor).isActive = true
            }
        }
        expandButton?.title = expanded ? "Show less" : "Show more"
    }

    private static func bodyTextField(_ text: NSAttributedString) -> NSTextField {
        let label = NSTextField(labelWithString: "")
        label.attributedStringValue = text
        label.lineBreakMode = .byWordWrapping
        label.maximumNumberOfLines = 0
        label.isSelectable = true
        label.allowsEditingTextAttributes = true  // clickable links
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return label
    }

    @objc private func toggleExpanded() {
        expanded.toggle()
        populateBody()
    }

    private static func truncate(_ text: NSAttributedString, toLines limit: Int) -> NSAttributedString {
        let string = text.string as NSString
        var newlines = 0
        var index = 0
        while index < string.length {
            if string.character(at: index) == 0x0A {
                newlines += 1
                if newlines == limit {
                    break
                }
            }
            index += 1
        }
        guard index < string.length else { return text }
        let out = text.attributedSubstring(from: NSRange(location: 0, length: index))
            .mutableCopy() as! NSMutableAttributedString
        out.append(
            NSAttributedString(
                string: "\n\u{2026}",
                attributes: [
                    .font: ThreadStyle.bodyFont,
                    .foregroundColor: NSColor.secondaryLabelColor,
                ]))
        return out
    }

    private static func verdictColor(_ verdict: String) -> NSColor {
        switch verdict {
        case "approved": return .systemGreen
        case "changes requested": return .systemRed
        default: return .secondaryLabelColor
        }
    }

    @objc private func copyClicked() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(entry.copyText, forType: .string)
        guard let copyButton else { return }
        Self.setLinkTitle(copyButton, "Copied!", color: .systemGreen)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak copyButton] in
            guard let copyButton else { return }
            Self.setLinkTitle(copyButton, "Copy", color: .secondaryLabelColor)
        }
    }

    @objc private func openClicked() {
        guard let url = entry.url.flatMap(URL.init(string:)) else { return }
        NSWorkspace.shared.open(url)
    }

    // ------------------------------------------------------------------
    // Inline edit

    @objc private func editClicked() {
        guard editBox == nil else { return }
        bodyStack.isHidden = true

        let scroll = NSTextView.scrollableTextView()
        let textView = scroll.documentView as! NSTextView
        textView.string = entry.body
        textView.font = ThreadStyle.bodyFont
        textView.isRichText = false
        textView.textContainerInset = NSSize(width: 4, height: 5)
        scroll.borderType = .bezelBorder
        scroll.hasVerticalScroller = false

        let cancel = NSButton(
            title: "Cancel", target: self, action: #selector(cancelEditClicked))
        cancel.bezelStyle = .rounded
        cancel.controlSize = .small
        cancel.keyEquivalent = "\u{1b}"
        let save = NSButton(title: "Save", target: self, action: #selector(saveEditClicked))
        save.bezelStyle = .rounded
        save.controlSize = .small
        save.keyEquivalent = "\r"
        save.keyEquivalentModifierMask = [.command]

        let buttons = NSStackView(views: [NSView(), cancel, save])
        buttons.orientation = .horizontal
        buttons.spacing = Metrics.padding

        let box = NSStackView(views: [scroll, buttons])
        box.orientation = .vertical
        box.alignment = .leading
        box.spacing = Metrics.unit + 2
        contentColumn.addArrangedSubview(box)
        let measured = entry.body.height(font: ThreadStyle.bodyFont, width: 300) + 16
        NSLayoutConstraint.activate([
            scroll.heightAnchor.constraint(equalToConstant: min(max(measured, 60), 220)),
            scroll.widthAnchor.constraint(equalTo: box.widthAnchor),
            buttons.widthAnchor.constraint(equalTo: box.widthAnchor),
            box.widthAnchor.constraint(equalTo: contentColumn.widthAnchor),
        ])
        editBox = box
        editTextView = textView
        window?.makeFirstResponder(textView)
    }

    @objc private func cancelEditClicked() {
        editBox?.removeFromSuperview()
        editBox = nil
        editTextView = nil
        bodyStack.isHidden = false
    }

    @objc private func saveEditClicked() {
        guard let textView = editTextView else { return }
        let body = textView.string.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty, body != entry.body else {
            cancelEditClicked()
            return
        }
        // Optimistic: show the new text now; the refetch confirms it.
        segments = MarkdownBody.render(body)
        populateBody()
        cancelEditClicked()
        onEdit?(entry, body)
    }
}

// ------------------------------------------------------------------
// Inline body image: async-loaded, aspect-sized, GIFs animate. Native
// all the way down — no webviews.

final class BodyImageView: NSImageView {
    init(url: URL, alt: String) {
        super.init(frame: .zero)
        imageScaling = .scaleProportionallyDown
        animates = true
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.masksToBounds = true
        layer?.backgroundColor = NSColor.quaternarySystemFill.cgColor
        toolTip = alt.isEmpty ? url.absoluteString : alt
        translatesAutoresizingMaskIntoConstraints = false

        let placeholderHeight = heightAnchor.constraint(equalToConstant: 96)
        let placeholderWidth = widthAnchor.constraint(equalToConstant: 200)
        placeholderWidth.priority = .defaultLow
        NSLayoutConstraint.activate([placeholderHeight, placeholderWidth])

        AvatarStore.shared.image(for: url.absoluteString) { [weak self] image in
            guard let self, let image, image.size.width > 0 else { return }
            self.image = image
            placeholderHeight.isActive = false
            placeholderWidth.isActive = false
            self.layer?.backgroundColor = NSColor.clear.cgColor
            // Natural size, capped by the card width (outer ≤ pin);
            // height follows the aspect ratio so nothing letterboxes.
            let natural = self.widthAnchor.constraint(equalToConstant: image.size.width)
            natural.priority = .defaultLow
            let aspect = self.heightAnchor.constraint(
                equalTo: self.widthAnchor,
                multiplier: image.size.height / image.size.width)
            aspect.priority = .init(999)
            NSLayoutConstraint.activate([natural, aspect])
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
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
        textView.font = ThreadStyle.bodyFont
        textView.isRichText = false
        textView.textContainerInset = NSSize(width: 4, height: 4)
        textView.delegate = self
        scroll.borderType = .bezelBorder
        scroll.hasVerticalScroller = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)

        // Size to content (within reason) — the pane scrolls, not the note.
        let measured = note.body.height(font: ThreadStyle.bodyFont, width: 300) + 14
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

        textView.font = ThreadStyle.bodyFont
        textView.isRichText = false
        textView.textContainerInset = NSSize(width: 6, height: 6)
        textScroll.borderType = .bezelBorder
        textScroll.hasVerticalScroller = false

        saveButton.bezelStyle = .rounded
        saveButton.controlSize = .regular
        saveButton.font = .systemFont(ofSize: 13)
        saveButton.keyEquivalent = "\r"
        saveButton.keyEquivalentModifierMask = [.command]
        saveButton.target = self
        saveButton.action = #selector(saveClicked(_:))

        hint.font = ThreadStyle.metaFont
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
            textScroll.heightAnchor.constraint(equalToConstant: 68),
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
