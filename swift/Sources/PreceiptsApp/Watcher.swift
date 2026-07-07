// FSEvents watcher: the "always current" invariant. Events are debounced
// by FSEvents latency, filtered for noise dirs, and coalesced by the
// caller's single-flight reload (the TUI process-storm lesson).

import CoreServices
import Foundation

final class Watcher {
    private var stream: FSEventStreamRef?
    private let onChange: () -> Void

    private static let noise: Set<String> = [
        ".git", "target", "node_modules", ".turbo", "dist", ".next", ".build",
    ]

    init?(workdir: URL, onChange: @escaping () -> Void) {
        self.onChange = onChange

        var context = FSEventStreamContext(
            version: 0,
            info: Unmanaged.passUnretained(self).toOpaque(),
            retain: nil,
            release: nil,
            copyDescription: nil
        )
        let callback: FSEventStreamCallback = { _, info, count, paths, _, _ in
            guard let info else { return }
            let watcher = Unmanaged<Watcher>.fromOpaque(info).takeUnretainedValue()
            let pathList = Unmanaged<CFArray>.fromOpaque(paths).takeUnretainedValue()
                as? [String] ?? []
            let relevant = pathList.prefix(Int(count)).contains { path in
                !path.split(separator: "/").contains { Watcher.noise.contains(String($0)) }
            }
            if relevant {
                watcher.onChange()
            }
        }

        guard
            let stream = FSEventStreamCreate(
                nil,
                callback,
                &context,
                [workdir.path] as CFArray,
                FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
                0.4,
                FSEventStreamCreateFlags(
                    kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents)
            )
        else {
            return nil
        }
        self.stream = stream
        FSEventStreamSetDispatchQueue(stream, DispatchQueue.main)
        FSEventStreamStart(stream)
    }

    deinit {
        if let stream {
            FSEventStreamStop(stream)
            FSEventStreamInvalidate(stream)
            FSEventStreamRelease(stream)
        }
    }
}
