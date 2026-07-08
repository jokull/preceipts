// Dev puppeting channel: a distributed notification carries a one-line
// command the app executes. This is how an agent iterates on the UI with
// vision — `cacheDisplay` renders our own window to a PNG in-process, so
// no screen-recording permission is needed. Inert unless poked.
//
// Commands (object string of "is.solberg.preceipts.debug"):
//   shot <path>     render the main window into a PNG at <path>
//   feedback        toggle the thread pane
//   conversation    open the pane at the PR conversation
//   thread <n>      open the nth thread (feedback order, 0-based)

import AppKit

final class DebugBridge {
    static let notification = Notification.Name("is.solberg.preceipts.debug")
    private var observer: NSObjectProtocol?

    init(handler: @escaping (String) -> Void) {
        observer = DistributedNotificationCenter.default().addObserver(
            forName: Self.notification, object: nil, queue: .main
        ) { note in
            guard let command = note.object as? String else { return }
            handler(command)
        }
    }

    deinit {
        if let observer {
            DistributedNotificationCenter.default().removeObserver(observer)
        }
    }

    /// Render a window (chrome included) into a PNG. Capturing our OWN
    /// window via CGWindowList needs no screen-recording permission;
    /// cacheDisplay can't compose layer-backed/material panes.
    static func screenshot(window: NSWindow, to path: String) {
        let windowID = CGWindowID(window.windowNumber)
        guard
            let image = CGWindowListCreateImage(
                .null, .optionIncludingWindow, windowID,
                [.boundsIgnoreFraming, .bestResolution]),
            image.width > 1
        else {
            screenshotViaLayer(window: window, to: path)
            return
        }
        let rep = NSBitmapImageRep(cgImage: image)
        guard let png = rep.representation(using: .png, properties: [:]) else { return }
        try? png.write(to: URL(fileURLWithPath: path))
    }

    /// Fallback: render the CA layer tree (materials come out blank).
    private static func screenshotViaLayer(window: NSWindow, to path: String) {
        guard let frame = window.contentView?.superview, let layer = frame.layer else { return }
        let scale = window.backingScaleFactor
        let size = frame.bounds.size
        guard
            let rep = NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale),
                bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
            let context = NSGraphicsContext(bitmapImageRep: rep)
        else { return }
        rep.size = size
        context.cgContext.scaleBy(x: scale, y: scale)
        layer.render(in: context.cgContext)
        guard let png = rep.representation(using: .png, properties: [:]) else { return }
        try? png.write(to: URL(fileURLWithPath: path))
    }
}
