import XCTest
@testable import RivalKit

/// The line-picking summary reader must agree with a full decode, and fall
/// back to one when the file is not laid out one field per line.
final class SessionSummaryTests: XCTestCase {
    let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.userInfo[Session.skipPromptKey] = true
        return d
    }()

    /// The CLI's indented JSON layout: prompt right after effort.
    func cliJSON(prompt: String, error: String = "boom \"quoted\" \\ back") -> String {
        let p = String(decoding: try! JSONEncoder().encode(prompt), as: UTF8.self)
        let e = String(decoding: try! JSONEncoder().encode(error), as: UTF8.self)
        return """
        {
          "id": "abc",
          "group_id": "g1",
          "cli": "codex",
          "mode": "review",
          "model": "gpt-6-astra",
          "effort": "high",
          "prompt": \(p),
          "prompt_preview": "review this",
          "status": "failed",
          "start_time": "2026-09-26T03:00:00.123456789Z",
          "end_time": "2026-09-26T03:05:00Z",
          "exit_code": 1,
          "duration": "5m0s",
          "work_dir": "/src/rival",
          "log_file": "/tmp/abc.log",
          "output_bytes": 1234,
          "output_lines": 56,
          "error": \(e),
          "pid": 4242,
          "pid_start": 1786708476373568000
        }

        """
    }

    func write(_ body: String) throws -> (String, Int64) {
        let url = try makeTempDir().appendingPathComponent("abc.json")
        try Data(body.utf8).write(to: url)
        return (url.path, Int64(body.utf8.count))
    }

    func assertMatchesFullDecode(_ body: String, file: StaticString = #filePath, line: UInt = #line) throws {
        let (path, size) = try write(body)
        let got = try XCTUnwrap(SessionSummary.load(path: path, size: size, decoder: decoder), file: file, line: line)
        let want = try decoder.decode(Session.self, from: Data(body.utf8))
        XCTAssertEqual(got, want, file: file, line: line)
        XCTAssertNil(got.prompt, file: file, line: line)
        XCTAssertEqual(got.error, "boom \"quoted\" \\ back", file: file, line: line)
    }

    func testSmallFile() throws {
        try assertMatchesFullDecode(cliJSON(prompt: "short"))
    }

    func testMidFileLineScan() throws {
        // Over lineScanMin, under the edge threshold; a quote-laden prompt
        // with "key": lookalikes must not leak into the summary.
        let prompt = String(repeating: "\n  \"status\": \"running\",\n", count: 400)
        try assertMatchesFullDecode(cliJSON(prompt: prompt))
    }

    func testBigFileReadsEdges() throws {
        let prompt = String(repeating: "x", count: 3 * SessionSummary.edgeBytes)
        try assertMatchesFullDecode(cliJSON(prompt: prompt))
    }

    func testCompactJSONFallsBackToFullDecode() throws {
        let body = #"{"id":"abc","status":"completed","prompt":""# + String(repeating: "y", count: 10_000) + #""}"#
        let (path, size) = try write(body)
        let s = try XCTUnwrap(SessionSummary.load(path: path, size: size, decoder: decoder))
        XCTAssertEqual(s.id, "abc")
        XCTAssertEqual(s.status, "completed")
        XCTAssertNil(s.prompt)
    }

    /// A prompt line that is not valid JSON makes a full decode throw, so a
    /// result proves the prompt line was never parsed.
    func testPromptLineIsNeverParsed() throws {
        for count in [5_000, 3 * SessionSummary.edgeBytes] {
            let half = String(repeating: "z", count: count / 2)
            let body = cliJSON(prompt: half + "TAB" + half).replacingOccurrences(of: "TAB", with: #"" garbage ""#)
            XCTAssertThrowsError(try decoder.decode(Session.self, from: Data(body.utf8)))
            let (path, size) = try write(body)
            let s = try XCTUnwrap(SessionSummary.load(path: path, size: size, decoder: decoder), "size \(count)")
            XCTAssertEqual(s.id, "abc")
            XCTAssertEqual(s.pidStart, 1786708476373568000)
        }
    }
}
