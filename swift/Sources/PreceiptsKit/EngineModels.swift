// Codable mirrors of the engine's JSON surfaces (engine/src/lib/{hud,
// status,receipt,events}.ts). The engine stays behind its subprocess
// boundary; these are the wire types for `hud --json` and `run --events`.

import Foundation

public struct EngineRunner: Codable, Sendable, Equatable {
    public let name: String
    public let email: String
    public let host: String
    public let agent: String?
}

/// One receipt: a single check run, keyed to a git tree.
public struct EngineReceipt: Codable, Sendable, Equatable {
    public let v: Int
    public let check: String
    public let cmd: String
    public let tree: String
    public let ok: Bool
    public let exit: Int
    public let started: String
    public let durationMs: Double
    public let runner: EngineRunner
    public let dirty: Bool
    /// "blob:<sha>" reference to the stored log.
    public let log: String
    public let checkBlob: String

    enum CodingKeys: String, CodingKey {
        case v, check, cmd, tree, ok, exit, started, runner, dirty, log
        case durationMs = "duration_ms"
        case checkBlob = "check_blob"
    }
}

public enum EngineCheckState: String, Codable, Sendable {
    case ok
    case fail
    case missing
    case staleDefinition = "stale-definition"
}

public struct EngineStatusRow: Codable, Sendable {
    public let check: String
    public let required: Bool
    public let state: EngineCheckState
    public let receipt: EngineReceipt?
}

public struct EngineStatus: Codable, Sendable {
    public let ref: String
    public let tree: String
    public let rows: [EngineStatusRow]
    /// True iff every required check has an ok receipt whose definition
    /// still matches.
    public let green: Bool
}

/// `preceipts-engine hud --json` — branch situational awareness.
public struct EngineHud: Codable, Sendable {
    public let branch: String?
    /// The ref conflicts/freshness were computed against (e.g. "origin/main").
    public let base: String
    public let head: String
    /// Working-tree hash — what `run` would mint against right now.
    public let tree: String
    public let dirty: Bool
    public let ahead: Int
    public let behind: Int
    /// nil when unanswerable (no merge base / old git).
    public let mergeClean: Bool?
    public let conflictFiles: [String]
    public let landFresh: Bool
    public let fetchAgeMs: Double?
    /// nil when there's no origin.
    public let unsyncedReceipts: Int?
    public let status: EngineStatus
    public let green: Bool
}

// ------------------------------------------------------------------
// `run --events` NDJSON stream

public enum EngineRunEvent: Sendable, Equatable {
    case prepareStarted(step: String)
    case prepareOutput(step: String, chunk: String)
    case prepareFinished(step: String, ok: Bool, exit: Int, durationMs: Double)
    /// Prepare changed the worktree; receipts key to the normalized tree.
    case treeNormalized(tree: String, changed: [String])
    case runStarted(tree: String, dirty: Bool)
    /// The worktree changed while checks ran — run invalid, nothing minted.
    case worktreeChanged(before: String, after: String, changed: [String])
    case checkStarted(check: String, tree: String)
    case output(check: String, chunk: String)
    case checkFinished(check: String, ok: Bool, exit: Int, durationMs: Double)
    case receiptMinted(check: String, tree: String, log: String)
}

extension EngineRunEvent: Decodable {
    private enum CodingKeys: String, CodingKey {
        case event, step, chunk, ok, exit, tree, changed, dirty, before, after
        case check, log, ts
        case durationMs = "duration_ms"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .event)
        switch kind {
        case "prepare-started":
            self = .prepareStarted(step: try container.decode(String.self, forKey: .step))
        case "prepare-output":
            self = .prepareOutput(
                step: try container.decode(String.self, forKey: .step),
                chunk: try container.decode(String.self, forKey: .chunk))
        case "prepare-finished":
            self = .prepareFinished(
                step: try container.decode(String.self, forKey: .step),
                ok: try container.decode(Bool.self, forKey: .ok),
                exit: try container.decode(Int.self, forKey: .exit),
                durationMs: try container.decode(Double.self, forKey: .durationMs))
        case "tree-normalized":
            self = .treeNormalized(
                tree: try container.decode(String.self, forKey: .tree),
                changed: try container.decode([String].self, forKey: .changed))
        case "run-started":
            self = .runStarted(
                tree: try container.decode(String.self, forKey: .tree),
                dirty: try container.decode(Bool.self, forKey: .dirty))
        case "worktree-changed":
            self = .worktreeChanged(
                before: try container.decode(String.self, forKey: .before),
                after: try container.decode(String.self, forKey: .after),
                changed: try container.decode([String].self, forKey: .changed))
        case "check-started":
            self = .checkStarted(
                check: try container.decode(String.self, forKey: .check),
                tree: try container.decode(String.self, forKey: .tree))
        case "output":
            self = .output(
                check: try container.decode(String.self, forKey: .check),
                chunk: try container.decode(String.self, forKey: .chunk))
        case "check-finished":
            self = .checkFinished(
                check: try container.decode(String.self, forKey: .check),
                ok: try container.decode(Bool.self, forKey: .ok),
                exit: try container.decode(Int.self, forKey: .exit),
                durationMs: try container.decode(Double.self, forKey: .durationMs))
        case "receipt-minted":
            self = .receiptMinted(
                check: try container.decode(String.self, forKey: .check),
                tree: try container.decode(String.self, forKey: .tree),
                log: try container.decode(String.self, forKey: .log))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .event, in: container,
                debugDescription: "unknown run event \"\(kind)\"")
        }
    }
}
