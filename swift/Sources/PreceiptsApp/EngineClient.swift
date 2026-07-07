// The engine stays behind its subprocess boundary (`preceipts-engine`,
// TS/Bun): `hud --json` for situational awareness, `run --events` for
// NDJSON-streamed check runs. Hud refreshes are single-flight coalesced
// (same discipline as diff reloads); runs are strictly one at a time —
// the engine's own run lock is the backstop, not the strategy.

import Foundation
import PreceiptsKit

final class EngineClient {
    private let workdir: URL
    private let binary: URL?

    private var hudInFlight = false
    private var hudQueued = false
    private var queuedBase: String?
    private(set) var runInProgress = false

    init(workdir: URL) {
        self.workdir = workdir
        self.binary = Self.locateBinary()
    }

    var available: Bool { binary != nil }

    /// PRECEIPTS_ENGINE env → ~/bin/preceipts-engine → $PATH.
    private static func locateBinary() -> URL? {
        let fm = FileManager.default
        if let override = ProcessInfo.processInfo.environment["PRECEIPTS_ENGINE"],
            fm.isExecutableFile(atPath: override)
        {
            return URL(fileURLWithPath: override)
        }
        let home = fm.homeDirectoryForCurrentUser.appendingPathComponent(
            "bin/preceipts-engine")
        if fm.isExecutableFile(atPath: home.path) {
            return home
        }
        let path = ProcessInfo.processInfo.environment["PATH"] ?? ""
        for dir in path.split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(dir))
                .appendingPathComponent("preceipts-engine")
            if fm.isExecutableFile(atPath: candidate.path) {
                return candidate
            }
        }
        return nil
    }

    // ------------------------------------------------------------------
    // Hud (single-flight, coalesced; completion on the main thread)

    func refreshHud(base: String?, completion: @escaping (Result<EngineHud, Error>) -> Void) {
        guard let binary else { return }
        if hudInFlight {
            hudQueued = true
            queuedBase = base
            return
        }
        hudInFlight = true
        let workdir = workdir
        DispatchQueue.global(qos: .utility).async { [weak self] in
            var arguments = ["hud", "--json"]
            if let base {
                arguments += ["--base", base]
            }
            let result = Self.capture(binary: binary, arguments: arguments, cwd: workdir)
                .flatMap { data in
                    Result { try JSONDecoder().decode(EngineHud.self, from: data) }
                }
            DispatchQueue.main.async {
                guard let self else { return }
                self.hudInFlight = false
                completion(result)
                if self.hudQueued {
                    self.hudQueued = false
                    self.refreshHud(base: self.queuedBase, completion: completion)
                }
            }
        }
    }

    /// Run a subprocess to completion, stdout on success, stderr as error.
    private static func capture(
        binary: URL, arguments: [String], cwd: URL
    ) -> Result<Data, Error> {
        let process = Process()
        process.executableURL = binary
        process.arguments = arguments
        process.currentDirectoryURL = cwd
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        do {
            try process.run()
        } catch {
            return .failure(error)
        }
        // Drain both pipes concurrently — a one-sided read deadlocks when
        // the other side fills its buffer.
        var errorOutput = Data()
        let drained = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .utility).async {
            errorOutput = stderr.fileHandleForReading.readDataToEndOfFile()
            drained.signal()
        }
        let output = stdout.fileHandleForReading.readDataToEndOfFile()
        drained.wait()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let message = String(decoding: errorOutput, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return .failure(
                EngineError.commandFailed(
                    exit: process.terminationStatus,
                    stderr: message.isEmpty ? "engine exited nonzero" : message))
        }
        return .success(output)
    }

    // ------------------------------------------------------------------
    // Run (streaming)

    /// Spawn `run --events`; events and completion arrive on the main
    /// thread. Returns false if a run is already in progress.
    @discardableResult
    func runChecks(
        onEvent: @escaping (EngineRunEvent) -> Void,
        completion: @escaping (RunOutcome) -> Void
    ) -> Bool {
        guard let binary, !runInProgress else { return false }
        runInProgress = true

        let process = Process()
        process.executableURL = binary
        process.arguments = ["run", "--events"]
        process.currentDirectoryURL = workdir
        var environment = ProcessInfo.processInfo.environment
        environment["PRECEIPTS_AGENT"] = environment["PRECEIPTS_AGENT"] ?? "preceipts.app"
        process.environment = environment

        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr

        // Line-buffered NDJSON decode off the readability handler.
        var buffer = Data()
        let decoder = JSONDecoder()
        stdout.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            guard !chunk.isEmpty else { return }
            buffer.append(chunk)
            while let newline = buffer.firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex..<newline]
                buffer.removeSubrange(buffer.startIndex...newline)
                guard !line.isEmpty else { continue }
                if let event = try? decoder.decode(EngineRunEvent.self, from: Data(line)) {
                    DispatchQueue.main.async { onEvent(event) }
                }
            }
        }

        process.terminationHandler = { [weak self] process in
            stdout.fileHandleForReading.readabilityHandler = nil
            let errorOutput = stderr.fileHandleForReading.readDataToEndOfFile()
            let note = String(decoding: errorOutput, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let outcome = RunOutcome(
                exit: process.terminationStatus,
                note: note.isEmpty ? nil : note)
            DispatchQueue.main.async {
                self?.runInProgress = false
                completion(outcome)
            }
        }

        do {
            try process.run()
            return true
        } catch {
            runInProgress = false
            stdout.fileHandleForReading.readabilityHandler = nil
            DispatchQueue.main.async {
                completion(RunOutcome(exit: -1, note: error.localizedDescription))
            }
            return false
        }
    }
}

struct RunOutcome {
    /// 0 = all checks passed, 1 = a check failed, else engine error
    /// (lock refusal, invalidated run — see `note`).
    let exit: Int32
    /// stderr tail — the run-level note line (lock refusal, worktree
    /// invalidation explanation, dirty warning).
    let note: String?
}

enum EngineError: LocalizedError {
    case commandFailed(exit: Int32, stderr: String)

    var errorDescription: String? {
        switch self {
        case .commandFailed(_, let stderr): return stderr
        }
    }
}
