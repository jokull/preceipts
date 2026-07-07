import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow!
    private var cockpit: CockpitViewController!

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
        window.center()
        window.setFrameAutosaveName("PreceiptsMainWindow")
        window.makeKeyAndOrderFront(nil)

        buildMenu()
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    private func buildMenu() {
        let mainMenu = NSMenu()

        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(
            withTitle: "About preceipts",
            action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
            keyEquivalent: ""
        )
        appMenu.addItem(.separator())
        appMenu.addItem(
            withTitle: "Quit preceipts",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)

        let viewMenuItem = NSMenuItem()
        let viewMenu = NSMenu(title: "View")
        let scopeItem = NSMenuItem(
            title: "Toggle Diff Scope",
            action: #selector(CockpitViewController.toggleScope(_:)),
            keyEquivalent: "d"
        )
        scopeItem.keyEquivalentModifierMask = [.command, .shift]
        viewMenu.addItem(scopeItem)
        let reloadItem = NSMenuItem(
            title: "Reload",
            action: #selector(CockpitViewController.reload(_:)),
            keyEquivalent: "r"
        )
        viewMenu.addItem(reloadItem)
        viewMenuItem.submenu = viewMenu
        mainMenu.addItem(viewMenuItem)

        NSApp.mainMenu = mainMenu
    }
}
