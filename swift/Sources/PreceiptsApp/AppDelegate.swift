import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow!
    private var cockpit: CockpitViewController!
    private lazy var settings = SettingsWindowController()

    @objc private func openSettings(_ sender: Any?) {
        settings.show()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let repoPath =
            CommandLine.arguments.dropFirst().first
            ?? FileManager.default.currentDirectoryPath
        let repo = URL(fileURLWithPath: repoPath)

        cockpit = CockpitViewController(repo: repo)

        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1380, height: 880),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = repo.lastPathComponent
        window.subtitle = "preceipts"
        window.contentViewController = cockpit
        window.toolbarStyle = .unified
        window.toolbar = cockpit.makeToolbar()
        window.center()
        window.setFrameAutosaveName("PreceiptsMainWindow")
        window.makeKeyAndOrderFront(nil)

        buildMenu()
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    // ------------------------------------------------------------------
    // Menus — every binding lives here so it's discoverable and
    // remappable the macOS way (docs/desktop-app-design.md keyboard map).

    private func buildMenu() {
        let mainMenu = NSMenu()
        mainMenu.addItem(appMenuItem())
        mainMenu.addItem(editMenuItem())
        mainMenu.addItem(viewMenuItem())
        mainMenu.addItem(goMenuItem())
        mainMenu.addItem(runMenuItem())
        NSApp.mainMenu = mainMenu
    }

    private func appMenuItem() -> NSMenuItem {
        let item = NSMenuItem()
        let menu = NSMenu()
        menu.addItem(
            withTitle: "About preceipts",
            action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
            keyEquivalent: "")
        menu.addItem(.separator())
        let settingsItem = NSMenuItem(
            title: "Settings\u{2026}",
            action: #selector(openSettings(_:)),
            keyEquivalent: ",")
        settingsItem.target = self
        menu.addItem(settingsItem)
        menu.addItem(.separator())
        menu.addItem(
            withTitle: "Quit preceipts",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q")
        item.submenu = menu
        return item
    }

    private func editMenuItem() -> NSMenuItem {
        let item = NSMenuItem()
        let menu = NSMenu(title: "Edit")
        // Standard text actions so the find field behaves like a Mac field.
        menu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        menu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        menu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        menu.addItem(
            withTitle: "Select All",
            action: #selector(NSText.selectAll(_:)),
            keyEquivalent: "a")
        menu.addItem(.separator())
        menu.addItem(
            withTitle: "Find\u{2026}",
            action: #selector(CockpitViewController.openFind(_:)),
            keyEquivalent: "f")
        menu.addItem(
            withTitle: "Find Next",
            action: #selector(CockpitViewController.findNext(_:)),
            keyEquivalent: "g")
        let previous = NSMenuItem(
            title: "Find Previous",
            action: #selector(CockpitViewController.findPrevious(_:)),
            keyEquivalent: "g")
        previous.keyEquivalentModifierMask = [.command, .shift]
        menu.addItem(previous)
        menu.addItem(.separator())
        let comment = NSMenuItem(
            title: "Add Comment\u{2026}",
            action: #selector(CockpitViewController.addComment(_:)),
            keyEquivalent: "m")
        comment.keyEquivalentModifierMask = [.command, .shift]
        menu.addItem(comment)
        item.submenu = menu
        return item
    }

    private func viewMenuItem() -> NSMenuItem {
        let item = NSMenuItem()
        let menu = NSMenu(title: "View")
        menu.addItem(
            withTitle: "Toggle Sidebar",
            action: #selector(NSSplitViewController.toggleSidebar(_:)),
            keyEquivalent: "b")
        menu.addItem(.separator())
        menu.addItem(
            withTitle: "Branch Diff",
            action: #selector(CockpitViewController.selectBranchScope(_:)),
            keyEquivalent: "1")
        menu.addItem(
            withTitle: "Uncommitted",
            action: #selector(CockpitViewController.selectUncommittedScope(_:)),
            keyEquivalent: "2")
        let toggleScope = NSMenuItem(
            title: "Toggle Diff Scope",
            action: #selector(CockpitViewController.toggleScope(_:)),
            keyEquivalent: "d")
        toggleScope.keyEquivalentModifierMask = [.command, .shift]
        menu.addItem(toggleScope)
        let receipts = NSMenuItem(
            title: "Toggle Receipts Panel",
            action: #selector(CockpitViewController.toggleReceiptsPanel(_:)),
            keyEquivalent: "j")
        menu.addItem(receipts)
        let feedback = NSMenuItem(
            title: "Toggle Feedback Panel",
            action: #selector(CockpitViewController.toggleFeedbackPanel(_:)),
            keyEquivalent: "j")
        feedback.keyEquivalentModifierMask = [.command, .shift]
        menu.addItem(feedback)
        menu.addItem(.separator())
        // ⌘R belongs to Run Checks (design keyboard map); watch mode makes
        // manual reload the exception.
        let reload = NSMenuItem(
            title: "Reload",
            action: #selector(CockpitViewController.reload(_:)),
            keyEquivalent: "r")
        reload.keyEquivalentModifierMask = [.command, .shift]
        menu.addItem(reload)
        item.submenu = menu
        return item
    }

    private func runMenuItem() -> NSMenuItem {
        let item = NSMenuItem()
        let menu = NSMenu(title: "Run")
        menu.addItem(
            withTitle: "Run Checks",
            action: #selector(CockpitViewController.runChecks(_:)),
            keyEquivalent: "r")
        item.submenu = menu
        return item
    }

    private func goMenuItem() -> NSMenuItem {
        let item = NSMenuItem()
        let menu = NSMenu(title: "Go")
        let bindings:
            [(String, Selector, String, NSEvent.ModifierFlags)] = [
                (
                    "Next File", #selector(CockpitViewController.nextFile(_:)),
                    "\u{F701}", [.command, .control]
                ),
                (
                    "Previous File", #selector(CockpitViewController.previousFile(_:)),
                    "\u{F700}", [.command, .control]
                ),
                (
                    "Next Hunk", #selector(CockpitViewController.nextHunk(_:)),
                    "\u{F701}", [.option]
                ),
                (
                    "Previous Hunk", #selector(CockpitViewController.previousHunk(_:)),
                    "\u{F700}", [.option]
                ),
            ]
        for (title, action, key, modifiers) in bindings {
            let entry = NSMenuItem(title: title, action: action, keyEquivalent: key)
            entry.keyEquivalentModifierMask = modifiers
            menu.addItem(entry)
        }
        item.submenu = menu
        return item
    }
}
