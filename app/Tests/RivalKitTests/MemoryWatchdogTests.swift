import Foundation
import XCTest
@testable import RivalKit

/// Records exit codes instead of ending the test process.
private final class ExitRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var codes: [Int32] = []

    func record(_ code: Int32) {
        lock.lock()
        codes.append(code)
        lock.unlock()
    }

    var recorded: [Int32] {
        lock.lock()
        defer { lock.unlock() }
        return codes
    }
}

final class MemoryWatchdogTests: XCTestCase {
    let limit: UInt64 = 1000

    func testJudgeFootprint() {
        XCTAssertEqual(judgeFootprint(nil, limit: limit), .unknown)
        XCTAssertEqual(judgeFootprint(limit - 1, limit: limit), .ok)
        XCTAssertEqual(judgeFootprint(limit, limit: limit), .over(bytes: limit))
        XCTAssertEqual(judgeFootprint(limit + 1, limit: limit), .over(bytes: limit + 1))
    }

    fileprivate func makeWatchdog(reading bytes: UInt64?, logURL: URL, exits: ExitRecorder) -> MemoryWatchdog {
        MemoryWatchdog(limit: limit, read: { bytes }, logURL: logURL, version: "9.9.9-test",
                       exit: { exits.record($0) })
    }

    func testOverLimitLogsOnceAndExitsOnce() throws {
        // A missing subdirectory: check() must create it.
        let logURL = try makeTempDir().appendingPathComponent("Logs/watchdog.log")
        let exits = ExitRecorder()
        let dog = makeWatchdog(reading: limit + 5, logURL: logURL, exits: exits)

        dog.check()
        XCTAssertEqual(exits.recorded, [70])
        let lines = try String(contentsOf: logURL, encoding: .utf8).split(separator: "\n")
        XCTAssertEqual(lines.count, 1)
        let line = try XCTUnwrap(lines.first)
        XCTAssertTrue(line.contains("footprint=\(limit + 5)"), String(line))
        XCTAssertTrue(line.contains("limit=\(limit)"), String(line))
        XCTAssertTrue(line.contains("version=9.9.9-test"), String(line))

        dog.check()
        XCTAssertEqual(exits.recorded, [70], "fires at most once")
        XCTAssertEqual(try String(contentsOf: logURL, encoding: .utf8).split(separator: "\n").count, 1)
    }

    func testUnderLimitDoesNothing() throws {
        let logURL = try makeTempDir().appendingPathComponent("watchdog.log")
        let exits = ExitRecorder()
        makeWatchdog(reading: limit - 1, logURL: logURL, exits: exits).check()
        XCTAssertEqual(exits.recorded, [])
        XCTAssertFalse(FileManager.default.fileExists(atPath: logURL.path))
    }

    func testUnknownFootprintDoesNothing() throws {
        let logURL = try makeTempDir().appendingPathComponent("watchdog.log")
        let exits = ExitRecorder()
        makeWatchdog(reading: nil, logURL: logURL, exits: exits).check()
        XCTAssertEqual(exits.recorded, [])
        XCTAssertFalse(FileManager.default.fileExists(atPath: logURL.path))
    }

    func testCurrentFootprintReadsThisProcess() throws {
        let bytes = try XCTUnwrap(currentFootprint())
        XCTAssertGreaterThan(bytes, 1 << 20)
    }
}
