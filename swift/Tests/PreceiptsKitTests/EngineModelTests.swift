// Decoding the engine's wire formats — fixtures mirror what
// `preceipts-engine hud --json` and `run --events` actually emit
// (engine/src/lib/{hud,status,receipt,events}.ts).

import XCTest

@testable import PreceiptsKit

final class EngineModelTests: XCTestCase {
    func testDecodeHud() throws {
        let json = """
            {"branch":"feature","base":"origin/main","head":"abc123","tree":"def456",
             "dirty":true,"ahead":3,"behind":1,"mergeClean":false,
             "conflictFiles":["src/a.ts","src/b.ts"],"landFresh":false,
             "fetchAgeMs":523000.5,"unsyncedReceipts":2,
             "status":{"ref":"worktree","tree":"def456","green":false,"rows":[
               {"check":"test","required":true,"state":"ok","receipt":
                 {"v":1,"check":"test","cmd":"bun test","tree":"def456","ok":true,
                  "exit":0,"started":"2026-07-07T12:00:00Z","duration_ms":8123,
                  "runner":{"name":"j","email":"j@x.is","host":"mac","agent":"codex"},
                  "dirty":false,"log":"blob:aa11","check_blob":"bb22"}},
               {"check":"lint","required":true,"state":"missing","receipt":null},
               {"check":"i18n","required":false,"state":"stale-definition","receipt":null}
             ]},"green":false}
            """
        let hud = try JSONDecoder().decode(EngineHud.self, from: Data(json.utf8))
        XCTAssertEqual(hud.branch, "feature")
        XCTAssertEqual(hud.base, "origin/main")
        XCTAssertEqual(hud.ahead, 3)
        XCTAssertEqual(hud.behind, 1)
        XCTAssertEqual(hud.mergeClean, false)
        XCTAssertEqual(hud.conflictFiles.count, 2)
        XCTAssertEqual(hud.unsyncedReceipts, 2)
        XCTAssertFalse(hud.green)
        XCTAssertEqual(hud.status.rows.count, 3)
        let test = hud.status.rows[0]
        XCTAssertEqual(test.state, .ok)
        XCTAssertEqual(test.receipt?.runner.agent, "codex")
        XCTAssertEqual(test.receipt?.durationMs, 8123)
        XCTAssertEqual(hud.status.rows[1].state, .missing)
        XCTAssertNil(hud.status.rows[1].receipt)
        XCTAssertEqual(hud.status.rows[2].state, .staleDefinition)
    }

    func testDecodeHudNullables() throws {
        let json = """
            {"branch":null,"base":"main","head":"abc","tree":"def","dirty":false,
             "ahead":0,"behind":0,"mergeClean":null,"conflictFiles":[],
             "landFresh":true,"fetchAgeMs":null,"unsyncedReceipts":null,
             "status":{"ref":"worktree","tree":"def","green":true,"rows":[]},
             "green":true}
            """
        let hud = try JSONDecoder().decode(EngineHud.self, from: Data(json.utf8))
        XCTAssertNil(hud.branch)
        XCTAssertNil(hud.mergeClean)
        XCTAssertNil(hud.fetchAgeMs)
        XCTAssertNil(hud.unsyncedReceipts)
        XCTAssertTrue(hud.green)
    }

    func testDecodeRunEventStream() throws {
        let lines = [
            #"{"event":"prepare-started","step":"format","ts":"2026-07-07T12:00:00Z"}"#,
            #"{"event":"prepare-output","step":"format","chunk":"formatting...\n"}"#,
            #"{"event":"prepare-finished","step":"format","ok":true,"exit":0,"duration_ms":812}"#,
            #"{"event":"tree-normalized","tree":"aa","changed":["src/x.ts"]}"#,
            #"{"event":"run-started","tree":"aa","dirty":true}"#,
            #"{"event":"check-started","check":"test","tree":"aa","ts":"2026-07-07T12:00:01Z"}"#,
            #"{"event":"output","check":"test","chunk":"1 pass\n"}"#,
            #"{"event":"check-finished","check":"test","ok":false,"exit":1,"duration_ms":9000.5}"#,
            #"{"event":"receipt-minted","check":"test","tree":"aa","log":"blob:cc"}"#,
            #"{"event":"worktree-changed","before":"aa","after":"bb","changed":["gen.ts"]}"#,
        ]
        let decoder = JSONDecoder()
        let events = try lines.map {
            try decoder.decode(EngineRunEvent.self, from: Data($0.utf8))
        }
        XCTAssertEqual(events[0], .prepareStarted(step: "format"))
        XCTAssertEqual(events[1], .prepareOutput(step: "format", chunk: "formatting...\n"))
        XCTAssertEqual(
            events[2], .prepareFinished(step: "format", ok: true, exit: 0, durationMs: 812))
        XCTAssertEqual(events[3], .treeNormalized(tree: "aa", changed: ["src/x.ts"]))
        XCTAssertEqual(events[4], .runStarted(tree: "aa", dirty: true))
        XCTAssertEqual(events[5], .checkStarted(check: "test", tree: "aa"))
        XCTAssertEqual(events[6], .output(check: "test", chunk: "1 pass\n"))
        XCTAssertEqual(
            events[7], .checkFinished(check: "test", ok: false, exit: 1, durationMs: 9000.5))
        XCTAssertEqual(events[8], .receiptMinted(check: "test", tree: "aa", log: "blob:cc"))
        XCTAssertEqual(
            events[9], .worktreeChanged(before: "aa", after: "bb", changed: ["gen.ts"]))
    }

    func testUnknownRunEventThrows() {
        let json = #"{"event":"solar-flare"}"#
        XCTAssertThrowsError(
            try JSONDecoder().decode(EngineRunEvent.self, from: Data(json.utf8)))
    }
}
