import XCTest
@testable import RivalKit

/// Shared vectors with rival/internal/logfmt/logfmt_test.go.
final class LogTests: XCTestCase {
    // TestSanitize, verbatim (tabs survive this step).
    static let vectors: [(String, String, String)] = [
        ("tabs survive sanitize", "if x {\n\treturn\n}", "if x {\n\treturn\n}"),
        ("progress frames collapse to the last", "10%\r99%", "99%"),
        ("csi color stripped", "\u{1b}[31mred\u{1b}[0m", "red"),
        ("osc bel stripped", "\u{1b}]0;title\u{07}text", "text"),
        ("osc st stripped", "\u{1b}]8;;url\u{1b}\\link", "link"),
        ("backspace and nul dropped", "a\u{08}b\u{00}c", "abc"),
        ("newlines preserved", "one\ntwo\nthree", "one\ntwo\nthree"),
        ("plain text unchanged", "plain log line", "plain log line"),
        ("crlf lines survive", "alpha\r\nbeta\r\n", "alpha\nbeta\n"),
        ("trailing frame keeps its text", "working...\r", "working..."),
        ("crlf plus progress frames", "10%\r99%\r\n", "99%\n"),
    ]

    func testSanitizeKeepingTabsMatchesGo() {
        for (name, raw, want) in Self.vectors {
            XCTAssertEqual(sanitizeKeepingTabs(raw), want, name)
        }
    }

    func testSanitizeLogExpandsTabs() {
        for (name, raw, want) in Self.vectors {
            XCTAssertEqual(sanitizeLog(raw), want.replacingOccurrences(of: "\t", with: "    "), name)
        }
        XCTAssertEqual(sanitizeLog("if x {\n\treturn\n}"), "if x {\n    return\n}")
    }

    // TestExpandTabs
    func testExpandTabs() {
        XCTAssertEqual(expandTabs("a\tb", width: logTabWidth), "a    b")
        XCTAssertEqual(expandTabs("a\tb", width: 0), "ab")
        XCTAssertEqual(expandTabs("a\tb", width: -1), "ab")
    }

    func testMoreEscapes() {
        XCTAssertEqual(sanitizeLog("\u{1b}[1;38;2;255;0;0mbold\u{1b}[m"), "bold")
        XCTAssertEqual(sanitizeLog("\u{1b}(Bcharset"), "charset") // ESC + intermediate + final
        XCTAssertEqual(sanitizeLog("\u{1b}7save\u{1b}8"), "save")
        XCTAssertEqual(sanitizeLog("\u{1b}P1$r0m\u{1b}\\dcs"), "dcs")
        XCTAssertEqual(sanitizeLog("\u{1b}_apc payload\u{1b}\\after"), "after")
        XCTAssertEqual(sanitizeLog("del\u{7f}eted"), "deleted")
    }

    func testWideRunesSurvive() {
        let wide = "日本語 ✓ 🦀 résumé \u{1b}[32m緑\u{1b}[0m"
        XCTAssertEqual(sanitizeLog(wide), "日本語 ✓ 🦀 résumé 緑")
        XCTAssertEqual(sanitizeLog("⠋\tüber"), "⠋    über")
    }

    // MARK: readTail

    func write(_ bytes: [UInt8]) throws -> String {
        let dir = try makeTempDir()
        let url = dir.appendingPathComponent("run.log")
        try Data(bytes).write(to: url)
        return url.path
    }

    func testReadTailSmallFileIsWhole() throws {
        let path = try write(Array("one\ntwo\n".utf8))
        let (text, truncated) = try readTail(path: path, maxBytes: maxTailBytes)
        XCTAssertEqual(text, "one\ntwo\n")
        XCTAssertFalse(truncated)
    }

    func testReadTailAlignsPastFirstNewline() throws {
        let path = try write(Array("aaaa\nbbbb\ncccc\n".utf8))
        let (text, truncated) = try readTail(path: path, maxBytes: 8)
        // The last 8 bytes are "bb\ncccc\n"; the partial first line goes.
        XCTAssertEqual(text, "cccc\n")
        XCTAssertTrue(truncated)
    }

    func testReadTailCutMidRuneDropsHalfRune() throws {
        // One long line with no newline: the cut lands inside "é" (C3 A9) and
        // the orphan byte is dropped rather than rendered.
        let path = try write(Array("xxé12".utf8))
        let (text, truncated) = try readTail(path: path, maxBytes: 3)
        XCTAssertEqual(text, "12")
        XCTAssertTrue(truncated)
    }

    func testReadTailDropsInvalidUTF8() throws {
        let path = try write([0x61, 0xFF, 0x62, 0xC3, 0x28, 0x63])
        XCTAssertEqual(try readTail(path: path, maxBytes: 100).text, "ab(c")
    }

    func testReadTailMissingFileThrows() {
        XCTAssertThrowsError(try readTail(path: "/nonexistent/rival/\(UUID()).log", maxBytes: 10))
    }

    func testMaxTailBytesMatchesGo() {
        XCTAssertEqual(maxTailBytes, 262_144)
    }
}
