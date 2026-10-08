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
        XCTAssertTrue(s.startTime.isZeroTime)
    }

    /// A file from an older release: the zero time for every unset time and
    /// `\u003c`-style escapes. The zero time still means "unset".
    func testOldFileZeroTimesAndEscapes() throws {
        let json = #"""
        {"id":"x","group_id":"a\u003cb\u0026c\u003e","start_time":"0001-01-01T00:00:00Z",
         "queued_at":"0001-01-01T00:00:00Z","end_time":"0001-01-01T00:00:00Z"}
        """#
        let s = try JSONDecoder().decode(Session.self, from: Data(json.utf8))
        XCTAssertEqual(s.groupID, "a<b&c>")
        XCTAssertTrue(s.startTime.isZeroTime)
        XCTAssertTrue(s.queuedAt!.isZeroTime)
        XCTAssertTrue(s.endTime!.isZeroTime)
    }

    /// A file from this release: unset times are not written, and `<`, `>`,
    /// `&`, U+2028 are raw characters.
    func testNewFileOmittedTimesAndRawHTMLCharacters() throws {
        let json = "{\"id\":\"x\",\"group_id\":\"a<b&c>\",\"error\":\"<stderr> & \u{2028}\",\"status\":\"running\"}"
        let s = try JSONDecoder().decode(Session.self, from: Data(json.utf8))
        XCTAssertEqual(s.groupID, "a<b&c>")
        XCTAssertEqual(s.error, "<stderr> & \u{2028}")
        XCTAssertEqual(s.status, "running")
        XCTAssertTrue(s.startTime.isZeroTime)
        XCTAssertNil(s.queuedAt)
        XCTAssertNil(s.endTime)
    }

    func testMistypedFieldFallsBackToDefault() throws {
        let json = #"{"id":"x","pid":"not-a-number","start_time":"garbage","output_lines":3}"#
        let s = try JSONDecoder().decode(Session.self, from: Data(json.utf8))
        XCTAssertEqual(s.pid, 0)
        XCTAssertTrue(s.startTime.isZeroTime)
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

    // MARK: - Rust writer golden files (testdata/written)

    /// Every file the Rust writer produced decodes, and each field the app
    /// models matches the source JSON. `prompt_hash` is not modelled.
    func testRustWriterGoldenDecodes() throws {
        let names = try FileManager.default.contentsOfDirectory(atPath: writtenDir.path)
            .filter { $0.hasSuffix(".json") }.sorted()
        XCTAssertFalse(names.isEmpty, "no golden files in \(writtenDir.path)")
        for name in ["zero-times.json", "escapes.json", "minimal.json"] {
            XCTAssertTrue(names.contains(name), "missing \(name)")
        }

        for name in names {
            let data = try Data(contentsOf: writtenDir.appendingPathComponent(name))
            // Integers decode as Int64 here, never through Double.
            let raw = try JSONDecoder().decode(RawSession.self, from: data)
            let s = try JSONDecoder().decode(Session.self, from: data)
            func time(_ v: String?, _ key: String) -> Date? {
                guard let v else { return nil }
                let d = parseRFC3339(v)
                XCTAssertNotNil(d, "\(name): \(key) \(v)")
                return d
            }
            func nonEmpty(_ v: String?) -> String? { v?.isEmpty == false ? v : nil }
            func nonZero<T: BinaryInteger>(_ v: T?) -> T? { v == 0 ? nil : v }

            XCTAssertEqual(s.id, raw.id, name)
            XCTAssertEqual(s.groupID, nonEmpty(raw.group_id), name)
            XCTAssertEqual(s.cli, raw.cli ?? "", name)
            XCTAssertEqual(s.mode, raw.mode ?? "", name)
            XCTAssertEqual(s.model, raw.model ?? "", name)
            XCTAssertEqual(s.effort, raw.effort ?? "", name)
            XCTAssertEqual(s.reviewScope, nonEmpty(raw.review_scope), name)
            XCTAssertEqual(s.prompt, nonEmpty(raw.prompt), name)
            XCTAssertEqual(s.promptPreview, nonEmpty(raw.prompt_preview), name)
            XCTAssertEqual(s.status, raw.status ?? "", name)
            XCTAssertEqual(s.startTime, time(raw.start_time, "start_time") ?? .zeroTime, name)
            XCTAssertEqual(s.queuedAt, time(raw.queued_at, "queued_at"), name)
            XCTAssertEqual(s.queuePosition, raw.queue_position, name)
            XCTAssertEqual(s.endTime, time(raw.end_time, "end_time"), name)
            // A present exit_code 0 stays 0; only an absent key is nil.
            XCTAssertEqual(s.exitCode, raw.exit_code, name)
            XCTAssertEqual(s.duration, nonEmpty(raw.duration), name)
            XCTAssertEqual(s.workDir, raw.work_dir ?? "", name)
            XCTAssertEqual(s.logFile, raw.log_file ?? "", name)
            XCTAssertEqual(s.outputBytes, raw.output_bytes ?? 0, name)
            XCTAssertEqual(s.outputLines, raw.output_lines ?? 0, name)
            XCTAssertEqual(s.error, nonEmpty(raw.error), name)
            XCTAssertEqual(s.account, nonEmpty(raw.account), name)
            XCTAssertEqual(Int64(s.pid), raw.pid ?? 0, name)
            XCTAssertEqual(s.pidStart, nonZero(raw.pid_start), name)
            XCTAssertEqual(s.ownerPID.map(Int64.init), nonZero(raw.owner_pid), name)
            XCTAssertEqual(s.ownerPIDStart, nonZero(raw.owner_pid_start), name)
        }
    }

    func testRustWriterGoldenEdgeValues() throws {
        let z = try Session.load(from: writtenDir.appendingPathComponent("zero-times.json"))
        XCTAssertEqual(z.exitCode, 0)
        XCTAssertEqual(z.outputBytes, 9_007_199_254_740_993)
        XCTAssertEqual(z.pidStart, Int64.max)
        XCTAssertEqual(z.ownerPID, 2)
        XCTAssertEqual(z.ownerPIDStart, 1)
        // The writer omits unset times.
        XCTAssertTrue(z.startTime.isZeroTime)
        XCTAssertNil(z.queuedAt)
        XCTAssertNil(z.endTime)
        XCTAssertNil(z.groupID)
        XCTAssertNil(z.prompt)

        let m = try Session.load(from: writtenDir.appendingPathComponent("minimal.json"))
        XCTAssertNil(m.exitCode)
        XCTAssertNil(m.endTime)
        XCTAssertNil(m.pidStart)
        XCTAssertNil(m.ownerPID)

        let e = try Session.load(from: writtenDir.appendingPathComponent("escapes.json"))
        XCTAssertEqual(e.exitCode, -1)
        XCTAssertEqual(e.groupID, "grp<1>&2")
        XCTAssertEqual(e.prompt, "Check <script>alert(1)</script> & \"quotes\" \\ back\tslash\nnew\rline \u{01}\u{08}\u{0C}\u{1F}\u{7F} line\u{2028}sep para\u{2029}sep 🦀 é")
        XCTAssertEqual(e.error, "exit status 1: <stderr> & \u{2028}")
        XCTAssertNil(e.pidStart)
        // -03:30 offset, 1 ns fraction.
        XCTAssertEqual(e.startTime.timeIntervalSince1970, 1_790_825_399, accuracy: 1e-6)
    }

    func testRFC3339Parser() {
        XCTAssertEqual(parseRFC3339("1970-01-01T00:00:00Z")?.timeIntervalSince1970, 0)
        XCTAssertEqual(parseRFC3339("1970-01-01T08:00:00+08:00")?.timeIntervalSince1970, 0)
        XCTAssertEqual(parseRFC3339("1969-12-31T19:00:00-05:00")?.timeIntervalSince1970, 0)
        XCTAssertEqual(parseRFC3339("2000-02-29T12:00:00.5Z")?.timeIntervalSince1970, 951_825_600.5)
        XCTAssertEqual(parseRFC3339("0001-01-01T00:00:00Z"), Date.zeroTime)
        XCTAssertNil(parseRFC3339("2026-09-26"))
        XCTAssertNil(parseRFC3339("2026-09-26T10:00:00"))
        XCTAssertNil(parseRFC3339("2026-09-26T10:00:00.Z"))
        XCTAssertNil(parseRFC3339("2026-13-26T10:00:00Z"))
        XCTAssertNil(parseRFC3339("2026-09-26T10:00:00Zjunk"))
    }
}

/// `<repo>/testdata/written`: the Rust writer's golden session files.
private let writtenDir = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .appendingPathComponent("testdata/written", isDirectory: true)

/// The source JSON with strict types, read independently of `Session`.
/// Integers stay `Int64`, so `pid_start` = `Int64.max` and `output_bytes`
/// above 2^53 compare exactly.
private struct RawSession: Decodable {
    let id: String
    let group_id, cli, mode, model, effort, review_scope, prompt, prompt_preview: String?
    let status, start_time, queued_at, end_time, duration, work_dir, log_file, error, account: String?
    let queue_position, exit_code, output_lines: Int?
    let output_bytes, pid, pid_start, owner_pid, owner_pid_start: Int64?
}
