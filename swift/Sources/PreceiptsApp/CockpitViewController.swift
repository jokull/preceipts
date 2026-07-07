// The cockpit: file sidebar + one scroll surface + statusbar. State and
// loading discipline (single-flight coalesced reloads) live here.

import AppKit
import PreceiptsKit

final class CockpitViewController: NSViewController {
    private let repo: URL
    private var scope: DiffScope = .branch
    private var changeset: Changeset?
    private var surface: [SurfaceRow] = []
    private var fileAnchors: [Int] = []

    private var reloadInFlight = false
    private var reloadQueued = false
    private var watcher: Watcher?

    private let sidebarTable = NSTableView()
    private let surfaceTable = NSTableView()
    private let statusLeft = NSTextField(labelWithString: "loading\u{2026}")
    private let statusRight = NSTextField(labelWithString: "")

    init(repo: URL) {
        self.repo = repo
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    // ------------------------------------------------------------------
    // View construction

    override func loadView() {
        let root = NSView()
        root.wantsLayer = true
        root.layer?.backgroundColor = Theme.bg.cgColor

        let sidebarScroll = makeTable(
            sidebarTable, identifier: "files", delegate: self, dataSource: self)
        sidebarTable.action = #selector(sidebarClicked(_:))
        sidebarTable.target = self
        sidebarScroll.hasVerticalScroller = true

        let surfaceScroll = makeTable(
            surfaceTable, identifier: "surface", delegate: self, dataSource: self)
        surfaceScroll.hasVerticalScroller = true

        let split = NSSplitView()
        split.isVertical = true
        split.dividerStyle = .thin
        split.addArrangedSubview(sidebarScroll)
        split.addArrangedSubview(surfaceScroll)
        split.setHoldingPriority(.defaultHigh, forSubviewAt: 0)

        let statusBar = NSView()
        statusBar.wantsLayer = true
        statusBar.layer?.backgroundColor = Theme.bgPanel.cgColor
        for label in [statusLeft, statusRight] {
            label.font = NSFont.monospacedSystemFont(ofSize: 10.5, weight: .regular)
            label.textColor = Theme.fgMuted
            label.lineBreakMode = .byTruncatingTail
            label.translatesAutoresizingMaskIntoConstraints = false
            statusBar.addSubview(label)
        }
        statusLeft.textColor = Theme.accent

        split.translatesAutoresizingMaskIntoConstraints = false
        statusBar.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(split)
        root.addSubview(statusBar)

        NSLayoutConstraint.activate([
            split.topAnchor.constraint(equalTo: root.topAnchor),
            split.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            split.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            split.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            statusBar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            statusBar.heightAnchor.constraint(equalToConstant: 24),
            statusLeft.leadingAnchor.constraint(equalTo: statusBar.leadingAnchor, constant: 10),
            statusLeft.centerYAnchor.constraint(equalTo: statusBar.centerYAnchor),
            statusRight.trailingAnchor.constraint(
                equalTo: statusBar.trailingAnchor, constant: -10),
            statusRight.centerYAnchor.constraint(equalTo: statusBar.centerYAnchor),
            statusLeft.trailingAnchor.constraint(
                lessThanOrEqualTo: statusRight.leadingAnchor, constant: -12),
        ])

        self.view = root
        DispatchQueue.main.async { [weak self] in
            guard let self, let split = self.view.subviews.first as? NSSplitView else { return }
            split.setPosition(260, ofDividerAt: 0)
        }
    }

    private func makeTable(
        _ table: NSTableView,
        identifier: String,
        delegate: NSTableViewDelegate,
        dataSource: NSTableViewDataSource
    ) -> NSScrollView {
        let column = NSTableColumn(
            identifier: NSUserInterfaceItemIdentifier(rawValue: identifier))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = Theme.rowHeight
        table.intercellSpacing = .zero
        table.backgroundColor = identifier == "files" ? Theme.bgPanel : Theme.bg
        table.selectionHighlightStyle = .regular
        table.style = .plain
        table.delegate = delegate
        table.dataSource = dataSource
        table.allowsEmptySelection = true

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.drawsBackground = true
        scroll.backgroundColor = table.backgroundColor
        // The column tracks the table width so rows always fill.
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        column.width = 800
        return scroll
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        requestReload()
    }

    // ------------------------------------------------------------------
    // Loading (single-flight, coalesced — the TUI storm lesson)

    @objc func reload(_ sender: Any?) {
        requestReload()
    }

    @objc func toggleScope(_ sender: Any?) {
        scope = scope == .branch ? .uncommitted : .branch
        requestReload()
    }

    private func requestReload() {
        if reloadInFlight {
            reloadQueued = true
            return
        }
        reloadInFlight = true
        let repo = repo
        let scope = scope
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = Result { try ChangesetLoader.load(repoPath: repo, scope: scope) }
            DispatchQueue.main.async {
                guard let self else { return }
                self.reloadInFlight = false
                self.apply(result)
                if self.reloadQueued {
                    self.reloadQueued = false
                    self.requestReload()
                }
            }
        }
    }

    private func apply(_ result: Result<Changeset, Error>) {
        switch result {
        case .success(let changeset):
            self.changeset = changeset
            rebuildSurface(changeset)
            ensureWatcher(changeset)
            sidebarTable.reloadData()
            surfaceTable.reloadData()
            updateStatusBar(changeset)
        case .failure(let error):
            statusLeft.stringValue = error.localizedDescription
        }
    }

    private func rebuildSurface(_ changeset: Changeset) {
        surface.removeAll()
        fileAnchors.removeAll()
        for (fileIndex, file) in changeset.files.enumerated() {
            fileAnchors.append(surface.count)
            surface.append(.fileHeader(file: fileIndex))
            for (hunkIndex, hunk) in file.hunks.enumerated() {
                if hunk.skippedBefore > 0 {
                    surface.append(.gap(skipped: hunk.skippedBefore))
                }
                for rowIndex in hunk.rows.indices {
                    surface.append(.line(file: fileIndex, hunk: hunkIndex, row: rowIndex))
                }
            }
        }
    }

    private func ensureWatcher(_ changeset: Changeset) {
        guard watcher == nil else { return }
        watcher = Watcher(workdir: changeset.workdir) { [weak self] in
            self?.requestReload()
        }
    }

    private func updateStatusBar(_ changeset: Changeset) {
        let scopeLabel: String
        switch changeset.scope {
        case .branch: scopeLabel = "Branch diff: \(changeset.baseName)"
        case .uncommitted: scopeLabel = "Uncommitted"
        }
        statusLeft.stringValue = "\(scopeLabel)   \(changeset.branch ?? "(detached)")"
        statusRight.stringValue =
            "\(changeset.files.count) files  +\(changeset.totalAdded) \u{2212}\(changeset.totalRemoved)   \u{2318}\u{21e7}D scope"
    }

    @objc private func sidebarClicked(_ sender: Any?) {
        let row = sidebarTable.clickedRow
        guard row >= 0, row < fileAnchors.count else { return }
        surfaceTable.scrollRowToVisible(fileAnchors[row])
        // Put the header at the top when there's room below.
        if let scroll = surfaceTable.enclosingScrollView {
            let y = CGFloat(fileAnchors[row]) * (Theme.rowHeight)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
            scroll.reflectScrolledClipView(scroll.contentView)
        }
    }
}

// ------------------------------------------------------------------
// Tables

extension CockpitViewController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int {
        if tableView === sidebarTable {
            return changeset?.files.count ?? 0
        }
        return surface.count
    }

    func tableView(
        _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
    ) -> NSView? {
        if tableView === sidebarTable {
            return sidebarCell(row: row)
        }
        return surfaceCell(row: row)
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        tableView === sidebarTable
    }

    private func sidebarCell(row: Int) -> NSView? {
        guard let file = changeset?.files[row] else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("file-cell")
        let cell =
            sidebarTable.makeView(withIdentifier: identifier, owner: nil) as? NSTextField
            ?? {
                let field = NSTextField(labelWithString: "")
                field.identifier = identifier
                field.font = Theme.font
                field.lineBreakMode = .byTruncatingHead
                return field
            }()
        let text = NSMutableAttributedString()
        text.append(
            NSAttributedString(
                string: " \(file.status.rawValue) ",
                attributes: [
                    .font: Theme.font,
                    .foregroundColor: Theme.statusColor(file.status.rawValue),
                ]
            ))
        text.append(
            NSAttributedString(
                string: file.path,
                attributes: [.font: Theme.font, .foregroundColor: Theme.fg]
            ))
        cell.attributedStringValue = text
        return cell
    }

    private func surfaceCell(row: Int) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("diff-row")
        let cell =
            surfaceTable.makeView(withIdentifier: identifier, owner: nil) as? DiffRowView
            ?? {
                let view = DiffRowView()
                view.identifier = identifier
                return view
            }()
        cell.surfaceRow = surface[row]
        cell.changeset = changeset
        cell.needsDisplay = true
        return cell
    }
}
