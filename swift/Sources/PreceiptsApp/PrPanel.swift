// The pull-request drawer — the receipts panel's sibling. Opens from
// the HUD's PR chip (or ⌘⇧P): title + review state in the header, then
// three columns — description (rendered markdown), commit log, and the
// checks board where deployments (Vercel, Cloudflare…) live via
// statusCheckRollup. Chrome regime throughout.

import AppKit
import PreceiptsKit

final class PrPanelView: NSView {
    private let titleLabel = NSTextField(labelWithString: "Pull Request")
    private let openButton = NSButton()
    private let statusLabel = NSTextField(labelWithString: "")
    private let descriptionStack = NSStackView()
    private let commitsStack = NSStackView()
    private let checksStack = NSStackView()
    private let commitsHeader = NSTextField(labelWithString: "Commits")
    private let checksHeader = NSTextField(labelWithString: "Checks")
    private var overview: PrOverview?

    // ------------------------------------------------------------------
    // Construction

    init() {
        super.init(frame: .zero)

        let header = makeHeader()
        let body = makeBody()

        let stack = NSStackView(views: [header, body])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.distribution = .fill
        body.setContentHuggingPriority(.init(1), for: .vertical)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            header.heightAnchor.constraint(equalToConstant: 30),
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            body.widthAnchor.constraint(equalTo: stack.widthAnchor),
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

        titleLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        titleLabel.lineBreakMode = .byTruncatingTail
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        openButton.title = "Open on GitHub"
        openButton.bezelStyle = .accessoryBarAction
        openButton.controlSize = .small
        openButton.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        openButton.target = self
        openButton.action = #selector(openClicked(_:))

        for view in [topSeparator, bottomSeparator, titleLabel, openButton] as [NSView] {
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
            titleLabel.leadingAnchor.constraint(
                equalTo: bar.leadingAnchor, constant: Metrics.paddingWide),
            titleLabel.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
            openButton.trailingAnchor.constraint(
                equalTo: bar.trailingAnchor, constant: -Metrics.padding),
            openButton.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
            titleLabel.trailingAnchor.constraint(
                lessThanOrEqualTo: openButton.leadingAnchor, constant: -Metrics.padding),
        ])
        return bar
    }

    private func makeBody() -> NSView {
        statusLabel.font = .systemFont(ofSize: 12)
        statusLabel.textColor = .secondaryLabelColor

        let description = column(
            header: sectionHeader("Description"), stack: descriptionStack)
        let commits = column(header: commitsHeader, stack: commitsStack)
        let checks = column(header: checksHeader, stack: checksStack)
        for header in [commitsHeader, checksHeader] {
            styleSectionHeader(header)
        }

        let columns = NSStackView(views: [
            description, verticalRule(), commits, verticalRule(), checks,
        ])
        columns.orientation = .horizontal
        // Stretch children to the panel height — columns scroll inside.
        columns.alignment = .height
        columns.spacing = Metrics.paddingWide
        columns.setContentHuggingPriority(.init(1), for: .vertical)
        columns.edgeInsets = NSEdgeInsets(
            top: Metrics.paddingWide, left: Metrics.paddingXL,
            bottom: Metrics.padding, right: Metrics.paddingXL)

        NSLayoutConstraint.activate([
            commits.widthAnchor.constraint(equalToConstant: 320),
            checks.widthAnchor.constraint(equalToConstant: 280),
        ])

        let container = NSView()
        container.translatesAutoresizingMaskIntoConstraints = false
        columns.translatesAutoresizingMaskIntoConstraints = false
        statusLabel.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(columns)
        container.addSubview(statusLabel)
        NSLayoutConstraint.activate([
            columns.topAnchor.constraint(equalTo: container.topAnchor),
            columns.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            columns.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            columns.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            statusLabel.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            statusLabel.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])
        return container
    }

    /// A titled scrolling column: the panel is short; each column
    /// scrolls independently like Xcode report navigators.
    private func column(header: NSTextField, stack: NSStackView) -> NSView {
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = Metrics.padding

        let document = FlippedStackDocument(stack: stack)
        document.translatesAutoresizingMaskIntoConstraints = false
        let scroll = NSScrollView()
        scroll.documentView = document
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        NSLayoutConstraint.activate([
            document.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            document.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])

        let box = NSStackView(views: [header, scroll])
        box.orientation = .vertical
        box.alignment = .leading
        box.spacing = Metrics.unit + 2
        box.distribution = .fill
        scroll.setContentHuggingPriority(.init(1), for: .vertical)
        NSLayoutConstraint.activate([
            scroll.widthAnchor.constraint(equalTo: box.widthAnchor)
        ])
        return box
    }

    private func sectionHeader(_ title: String) -> NSTextField {
        let label = NSTextField(labelWithString: title)
        styleSectionHeader(label)
        return label
    }

    private func styleSectionHeader(_ label: NSTextField) {
        label.font = .systemFont(ofSize: 11, weight: .semibold)
        label.textColor = .secondaryLabelColor
    }

    private func verticalRule() -> NSView {
        let rule = NSBox()
        rule.boxType = .separator
        return rule
    }

    // ------------------------------------------------------------------
    // Content

    func showLoading() {
        statusLabel.stringValue = "Loading pull request\u{2026}"
        statusLabel.isHidden = false
    }

    func showEmpty(_ message: String) {
        statusLabel.stringValue = message
        statusLabel.isHidden = false
    }

    func apply(_ overview: PrOverview) {
        self.overview = overview
        statusLabel.isHidden = true

        var title = "#\(overview.number)  \(overview.title)"
        if let review = overview.reviewDecision {
            title += "  \u{00b7}  " + review.lowercased().replacingOccurrences(of: "_", with: " ")
        }
        titleLabel.stringValue = title
        titleLabel.toolTip = overview.title

        descriptionStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        let body = overview.body.trimmingCharacters(in: .whitespacesAndNewlines)
        if body.isEmpty {
            let empty = NSTextField(labelWithString: "No description")
            empty.font = ThreadStyle.bodyFont
            empty.textColor = .tertiaryLabelColor
            descriptionStack.addArrangedSubview(empty)
        } else {
            for segment in MarkdownBody.render(body) {
                switch segment {
                case .text(let text):
                    let label = ThreadStyle.bodyTextField(text)
                    descriptionStack.addArrangedSubview(label)
                    label.widthAnchor.constraint(
                        equalTo: descriptionStack.widthAnchor).isActive = true
                case .image(let url, let alt):
                    let view = BodyImageView(url: url, alt: alt)
                    descriptionStack.addArrangedSubview(view)
                    view.widthAnchor.constraint(
                        lessThanOrEqualTo: descriptionStack.widthAnchor).isActive = true
                }
            }
        }

        commitsHeader.stringValue = "Commits (\(overview.commits.count))"
        commitsStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for commit in overview.commits {
            let row = commitRow(commit)
            commitsStack.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: commitsStack.widthAnchor).isActive = true
        }

        checksHeader.stringValue = "Checks (\(overview.checks.count))"
        checksStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        checkLinks.removeAll()
        if overview.checks.isEmpty {
            let empty = NSTextField(labelWithString: "No checks reported")
            empty.font = ThreadStyle.metaFont
            empty.textColor = .tertiaryLabelColor
            checksStack.addArrangedSubview(empty)
        }
        for check in overview.checks {
            let row = checkRow(check)
            checksStack.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: checksStack.widthAnchor).isActive = true
        }
    }

    private func commitRow(_ commit: PrCommit) -> NSView {
        let sha = NSTextField(labelWithString: commit.sha)
        sha.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        sha.textColor = .secondaryLabelColor
        sha.setContentHuggingPriority(.required, for: .horizontal)
        sha.setContentCompressionResistancePriority(.required, for: .horizontal)

        let headline = NSTextField(labelWithString: commit.headline)
        headline.font = .systemFont(ofSize: 12)
        headline.lineBreakMode = .byTruncatingTail
        headline.toolTip = commit.headline
        headline.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let meta = NSTextField(
            labelWithString: "\(commit.author) \u{00b7} \(Self.relativeTime(commit.date))")
        meta.font = ThreadStyle.metaFont
        meta.textColor = .tertiaryLabelColor
        meta.lineBreakMode = .byTruncatingTail

        let top = NSStackView(views: [sha, headline])
        top.orientation = .horizontal
        top.spacing = 6
        let row = NSStackView(views: [top, meta])
        row.orientation = .vertical
        row.alignment = .leading
        row.spacing = 1
        NSLayoutConstraint.activate([
            top.widthAnchor.constraint(equalTo: row.widthAnchor),
            meta.leadingAnchor.constraint(
                equalTo: row.leadingAnchor, constant: sha.intrinsicContentSize.width + 6),
        ])
        return row
    }

    private func checkRow(_ check: PrCheck) -> NSView {
        let (symbol, tint) = Self.checkGlyph(check.state)
        let icon = NSImageView(
            image: NSImage(systemSymbolName: symbol, accessibilityDescription: nil) ?? NSImage())
        icon.symbolConfiguration = .init(pointSize: 11, weight: .medium)
        icon.contentTintColor = tint
        icon.setContentHuggingPriority(.required, for: .horizontal)

        let name = NSTextField(labelWithString: check.name)
        name.font = .systemFont(ofSize: 12)
        name.lineBreakMode = .byTruncatingTail
        name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        if let description = check.description, !description.isEmpty {
            name.toolTip = description
        }

        let row = NSStackView(views: [icon, name])
        if let details = check.detailsUrl, let url = URL(string: details) {
            let open = NSButton(title: "", target: self, action: #selector(checkLinkClicked(_:)))
            open.isBordered = false
            open.attributedTitle = NSAttributedString(
                string: "Details",
                attributes: [
                    .font: NSFont.systemFont(ofSize: 11),
                    .foregroundColor: NSColor.secondaryLabelColor,
                ])
            open.setContentHuggingPriority(.required, for: .horizontal)
            open.toolTip = url.absoluteString
            checkLinks[ObjectIdentifier(open)] = url
            row.addArrangedSubview(NSView())
            row.addArrangedSubview(open)
        }
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 6
        return row
    }

    private var checkLinks: [ObjectIdentifier: URL] = [:]

    private static func checkGlyph(_ state: PrCheck.State) -> (String, NSColor) {
        switch state {
        case .success: return ("checkmark.circle.fill", .systemGreen)
        case .failure: return ("xmark.circle.fill", .systemRed)
        case .pending: return ("clock.fill", .systemOrange)
        case .neutral: return ("minus.circle", .secondaryLabelColor)
        }
    }

    private static func relativeTime(_ iso: String) -> String {
        guard let date = ISO8601DateFormatter().date(from: iso) else { return "" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }

    // ------------------------------------------------------------------
    // Actions

    @objc private func openClicked(_ sender: Any?) {
        guard let url = overview.flatMap({ URL(string: $0.url) }) else { return }
        NSWorkspace.shared.open(url)
    }

    @objc private func checkLinkClicked(_ sender: NSButton) {
        guard let url = checkLinks[ObjectIdentifier(sender)] else { return }
        NSWorkspace.shared.open(url)
    }
}

/// Top-anchored scroll document for a stack (shared column shape).
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
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -Metrics.unit),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }
}
