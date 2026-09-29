import XCTest
@testable import RivalKit

final class SessionDecodingTests: XCTestCase {
    func testEveryFixtureDecodes() throws {
        let names = try fixtureNames()
        XCTAssertGreaterThanOrEqual(names.count, 10)
        for name in names {
            XCTAssertNoThrow(try loadFixture(name), name)
        }
    }

    func testSoloRunningFields() throws {
        let s = try loadFixture("solo-running.json")
        XCTAssertEqual(s.id, "a1b2c3d4-0001-4000-8000-000000000001")
        XCTAssertNil(s.groupID)
        XCTAssertEqual(s.cli, "codex")
        XCTAssertEqual(s.mode, "review")
        XCTAssertEqual(s.model, "gpt-6-astra")
        XCTAssertEqual(s.effort, "xhigh")
        XCTAssertEqual(s.reviewScope, "internal/core/")
        XCTAssertEqual(s.prompt, "Review the fingerprint re-key in internal/core.")
        XCTAssertEqual(s.status, "running")
        XCTAssertEqual(s.pid, 43952)
        XCTAssertEqual(s.pidStart, 1_790_392_198_373_568_000)
        XCTAssertNil(s.endTime)
        XCTAssertNil(s.exitCode)
        // 9-digit fraction, Z offset.
        XCTAssertEqual(s.startTime.timeIntervalSince1970, 1_790_392_200.123456789, accuracy: 1e-6)
        XCTAssertEqual(s.queuedAt!.timeIntervalSince1970, 1_790_392_198.5, accuracy: 1e-6)
    }

    func testQueuedAndFailed() throws {
        let q = try loadFixture("queued.json")
        XCTAssertEqual(q.status, "queued")
        XCTAssertEqual(q.queuePosition, 2)
        // +08:00 offset with a 6-digit fraction: 11:12 local = 03:12 UTC.
        XCTAssertEqual(q.startTime.timeIntervalSince1970, 1_790_392_320.018889, accuracy: 1e-6)

        let f = try loadFixture("failed.json")
        XCTAssertEqual(f.status, "failed")
        XCTAssertEqual(f.exitCode, 1)
        XCTAssertEqual(f.error, "codex: quota exceeded (429)")
        XCTAssertEqual(f.duration, "42s")
        XCTAssertNil(f.pidStart)
    }

    func testMegareviewGroupWithJudge() throws {
        let judge = try loadFixture("mega-judge.json")
        XCTAssertEqual(judge.groupID, "8a138d95-3176-4afb-aad1-a59e12b879c8")
        XCTAssertEqual(judge.mode, "consilium")
        XCTAssertEqual(judge.duration, "1m23s")
        XCTAssertEqual(judge.endTime!.timeIntervalSince1970 - judge.startTime.timeIntervalSince1970, 82.698722, accuracy: 1e-5)
    }

    func testSolHistory() throws {
        let s = try loadFixture("sol-history.json")
        XCTAssertEqual(s.model, "gpt-5.6-sol")
        XCTAssertEqual(s.mode, "raw")
        XCTAssertEqual(s.pidStart, 1_786_708_476_373_568_000)
    }

    func testMissingOptionalKeysUseDefaults() throws {
        let s = try loadFixture("minimal.json")
        XCTAssertEqual(s.id, "c0000000-0001-4000-8000-000000000001")
        XCTAssertEqual(s.mode, "")
        XCTAssertEqual(s.model, "")
        XCTAssertEqual(s.effort, "")
        XCTAssertEqual(s.workDir, "")
        XCTAssertEqual(s.logFile, "")
        XCTAssertEqual(s.outputBytes, 0)
        XCTAssertEqual(s.outputLines, 0)
        XCTAssertEqual(s.pid, 0)
        XCTAssertNil(s.pidStart)
        XCTAssertNil(s.prompt)
        XCTAssertNil(s.queuedAt)
        XCTAssertNil(s.queuePosition)
    }

    func testUnknownKeysAreIgnored() throws {
        let s = try loadFixture("unknown-key.json")
        XCTAssertEqual(s.cli, "grok")
        XCTAssertEqual(s.mode, "security")
        XCTAssertEqual(s.status, "completed")
        XCTAssertEqual(s.pid, 1234)
    }

    func testEmptyStringsAndZeroPidStartMeanUnset() throws {
        let json = #"{"id":"x","group_id":"","error":"","pid_start":0,"start_time":"0001-01-01T00:00:00Z"}"#
        let s = try JSONDecoder().decode(Session.self, from: Data(json.utf8))
        XCTAssertNil(s.groupID)
        XCTAssertNil(s.error)
        XCTAssertNil(s.pidStart)
        XCTAssertTrue(s.startTime.isGoZero)
    }

    func testMistypedFieldFallsBackToDefault() throws {
        let json = #"{"id":"x","pid":"not-a-number","start_time":"garbage","output_lines":3}"#
        let s = try JSONDecoder().decode(Session.self, from: Data(json.utf8))
        XCTAssertEqual(s.pid, 0)
        XCTAssertTrue(s.startTime.isGoZero)
        XCTAssertEqual(s.outputLines, 3)
    }

    func testMissingIDFails() {
        XCTAssertThrowsError(try JSONDecoder().decode(Session.self, from: Data(#"{"cli":"codex"}"#.utf8)))
        XCTAssertThrowsError(try JSONDecoder().decode(Session.self, from: Data("{not json".utf8)))
    }

    func testSkipPromptKeepsPreview() throws {
        let d = JSONDecoder()
        d.userInfo[Session.skipPromptKey] = true
        let data = try Data(contentsOf: fixturesDir.appendingPathComponent("solo-running.json"))
        let s = try d.decode(Session.self, from: data)
        XCTAssertNil(s.prompt)
        XCTAssertEqual(s.promptPreview, "Review the fingerprint re-key in internal/core.")
    }

    func testRFC3339Parser() {
        XCTAssertEqual(parseRFC3339("1970-01-01T00:00:00Z")?.timeIntervalSince1970, 0)
        XCTAssertEqual(parseRFC3339("1970-01-01T08:00:00+08:00")?.timeIntervalSince1970, 0)
        XCTAssertEqual(parseRFC3339("1969-12-31T19:00:00-05:00")?.timeIntervalSince1970, 0)
        XCTAssertEqual(parseRFC3339("2000-02-29T12:00:00.5Z")?.timeIntervalSince1970, 951_825_600.5)
        XCTAssertEqual(parseRFC3339("0001-01-01T00:00:00Z"), Date.goZero)
        XCTAssertNil(parseRFC3339("2026-09-26"))
        XCTAssertNil(parseRFC3339("2026-09-26T10:00:00"))
        XCTAssertNil(parseRFC3339("2026-09-26T10:00:00.Z"))
        XCTAssertNil(parseRFC3339("2026-13-26T10:00:00Z"))
        XCTAssertNil(parseRFC3339("2026-09-26T10:00:00Zjunk"))
    }
}
