// Line diff via libgit2's xdiff (patience + indent heuristic — the same
// lineage as git's histogram default), then the display algorithm's
// remaining passes: similarity-gated pairing and word-level intraline.
// Produces the fixed-height DiffHunk/DiffRow model directly.

import Clibgit2
import Foundation

private let contextLines: UInt32 = 3

/// Raw line events collected from libgit2, per hunk.
private final class Collector {
    struct Event {
        let origin: CChar
        let oldLine: Int
        let newLine: Int
        let text: String
    }

    struct RawHunk {
        let oldStart: Int
        let oldLines: Int
        var events: [Event] = []
    }

    var hunks: [RawHunk] = []
    var added = 0
    var removed = 0
}

/// Diff two file contents into render-ready hunks.
public func diffRows(old: String, new: String) -> (hunks: [DiffHunk], added: Int, removed: Int) {
    let collector = Collector()

    var options = git_diff_options()
    git_diff_options_init(&options, UInt32(GIT_DIFF_OPTIONS_VERSION))
    options.context_lines = contextLines
    options.flags |= GIT_DIFF_INDENT_HEURISTIC.rawValue | GIT_DIFF_PATIENCE.rawValue

    let payload = Unmanaged.passUnretained(collector).toOpaque()

    let hunkCallback: git_diff_hunk_cb = { _, hunk, payload in
        guard let hunk = hunk?.pointee, let payload else { return 0 }
        let collector = Unmanaged<Collector>.fromOpaque(payload).takeUnretainedValue()
        collector.hunks.append(
            Collector.RawHunk(oldStart: Int(hunk.old_start), oldLines: Int(hunk.old_lines))
        )
        return 0
    }

    let lineCallback: git_diff_line_cb = { _, _, line, payload in
        guard let line = line?.pointee, let payload else { return 0 }
        let origin = line.origin
        // Only content lines; skip EOFNL markers and headers.
        guard
            origin == CChar(UInt8(ascii: " ")) || origin == CChar(UInt8(ascii: "+"))
                || origin == CChar(UInt8(ascii: "-"))
        else {
            return 0
        }
        let collector = Unmanaged<Collector>.fromOpaque(payload).takeUnretainedValue()
        var text = ""
        if line.content_len > 0, let content = line.content {
            let data = Data(bytes: content, count: line.content_len)
            text = String(decoding: data, as: UTF8.self)
            if text.hasSuffix("\n") { text.removeLast() }
            if text.hasSuffix("\r") { text.removeLast() }
        }
        if origin == CChar(UInt8(ascii: "+")) { collector.added += 1 }
        if origin == CChar(UInt8(ascii: "-")) { collector.removed += 1 }
        guard !collector.hunks.isEmpty else { return 0 }
        collector.hunks[collector.hunks.count - 1].events.append(
            Collector.Event(
                origin: origin,
                oldLine: Int(line.old_lineno),
                newLine: Int(line.new_lineno),
                text: text
            )
        )
        return 0
    }

    let oldBytes = Array(old.utf8)
    let newBytes = Array(new.utf8)
    oldBytes.withUnsafeBytes { oldBuffer in
        newBytes.withUnsafeBytes { newBuffer in
            _ = git_diff_buffers(
                oldBuffer.baseAddress,
                oldBuffer.count,
                nil,
                newBuffer.baseAddress,
                newBuffer.count,
                nil,
                &options,
                nil,
                nil,
                hunkCallback,
                lineCallback,
                payload
            )
        }
    }

    // Assemble rows: runs of -/+ inside a hunk form change blocks that go
    // through similarity pairing + intraline.
    var hunks: [DiffHunk] = []
    var previousOldEnd = 1
    for raw in collector.hunks {
        var rows: [DiffRow] = []
        var pendingOld: [Collector.Event] = []
        var pendingNew: [Collector.Event] = []

        func flushBlock() {
            guard !pendingOld.isEmpty || !pendingNew.isEmpty else { return }
            let oldTexts = pendingOld.map(\.text)
            let newTexts = pendingNew.map(\.text)
            for pairing in pairBlock(oldLines: oldTexts, newLines: newTexts) {
                switch pairing {
                case .pair(let o, let n):
                    let oldEvent = pendingOld[o]
                    let newEvent = pendingNew[n]
                    let (oldChanged, newChanged) = wordDiff(
                        old: oldEvent.text, new: newEvent.text)
                    rows.append(
                        DiffRow(
                            kind: .change,
                            old: LineRef(number: oldEvent.oldLine, text: oldEvent.text),
                            new: LineRef(number: newEvent.newLine, text: newEvent.text),
                            oldChanged: oldChanged,
                            newChanged: newChanged
                        )
                    )
                case .removed(let o):
                    let event = pendingOld[o]
                    rows.append(
                        DiffRow(
                            kind: .removal,
                            old: LineRef(number: event.oldLine, text: event.text),
                            new: nil
                        )
                    )
                case .added(let n):
                    let event = pendingNew[n]
                    rows.append(
                        DiffRow(
                            kind: .addition,
                            old: nil,
                            new: LineRef(number: event.newLine, text: event.text)
                        )
                    )
                }
            }
            pendingOld.removeAll()
            pendingNew.removeAll()
        }

        for event in raw.events {
            switch event.origin {
            case CChar(UInt8(ascii: "-")):
                pendingOld.append(event)
            case CChar(UInt8(ascii: "+")):
                pendingNew.append(event)
            default:
                flushBlock()
                rows.append(
                    DiffRow(
                        kind: .context,
                        old: LineRef(number: event.oldLine, text: event.text),
                        new: LineRef(number: event.newLine, text: event.text)
                    )
                )
            }
        }
        flushBlock()

        hunks.append(
            DiffHunk(skippedBefore: max(0, raw.oldStart - previousOldEnd), rows: rows)
        )
        previousOldEnd = raw.oldStart + raw.oldLines
    }

    return (hunks, collector.added, collector.removed)
}
