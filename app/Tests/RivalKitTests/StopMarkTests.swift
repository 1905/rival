import Darwin
import XCTest
@testable import RivalKit

/// `markStopped` on real files in a temp root. Never ~/.rival.
final class StopMarkTests: XCTestCase {
    var root: URL!
    var dir: URL!

    override func setUpWithError() throws {
        root = try makeTempDir()
        dir = root.appendingPathComponent("sessions", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    func write(_ id: String, _ json: String) throws {
        try Data(json.utf8).write(to: dir.appendingPathComponent(id + ".json"))
    }

    func object(_ id: String) throws -> [String: Any] {
        let data = try Data(contentsOf: dir.appendingPathComponent(id + ".json"))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    func inode(_ id: String) -> UInt64 {
        var st = stat()
        stat(dir.appendingPathComponent(id + ".json").path, &st)
        return UInt64(st.st_ino)
    }

    let running = """
    {
      "id": "s1",
      "cli": "codex",
      "status": "running",
      "start_time": "2026-09-26T03:00:00Z",
      "prompt": "the full prompt stays",
      "prompt_hash": "9566779c",
      "owner_pid": 43897,
      "pid": 43952,
      "pid_start": 1790392198373568000,
      "some_future_key": {"nested": [1, 2, "x"]}
    }
    """

    func testMarkEditsOnlyTheStopKeys() throws {
        try write("s1", running)
        let now = try XCTUnwrap(parseRFC3339("2026-09-26T03:06:24.4Z"))
        XCTAssertEqual(markStopped(sessionIDs: ["s1"], root: root, now: now), ["s1"])

        let o = try object("s1")
        XCTAssertEqual(o["status"] as? String, "failed")
        XCTAssertEqual(o["exit_code"] as? Int, 1)
        XCTAssertEqual(o["error"] as? String, "killed (process already dead)")
        XCTAssertEqual(o["duration"] as? String, "6m24s")
        let end = try XCTUnwrap((o["end_time"] as? String).flatMap(parseRFC3339))
        XCTAssertEqual(end.timeIntervalSince1970, now.timeIntervalSince1970, accuracy: 0.001)

        // Everything else survives, including keys the app does not model.
        XCTAssertEqual(o["prompt"] as? String, "the full prompt stays")
        XCTAssertEqual(o["prompt_hash"] as? String, "9566779c")
        XCTAssertEqual(o["owner_pid"] as? Int, 43897)
        XCTAssertEqual((o["pid_start"] as? NSNumber)?.int64Value, 1_790_392_198_373_568_000)
        let nested = try XCTUnwrap(o["some_future_key"] as? [String: Any])
        XCTAssertEqual((nested["nested"] as? [Any])?.count, 3)
        XCTAssertEqual(Set(o.keys), [
            "id", "cli", "status", "start_time", "prompt", "prompt_hash", "owner_pid", "pid", "pid_start",
            "some_future_key", "exit_code", "error", "end_time", "duration",
        ])

        // The app's decoder reads the result.
        let s = try Session.load(from: dir.appendingPathComponent("s1.json"))
        XCTAssertEqual(s.status, "failed")
        XCTAssertEqual(s.exitCode, 1)
        XCTAssertEqual(s.pidStart, 1_790_392_198_373_568_000)
    }

    func testWriteIsAtomicRename() throws {
        try write("s1", running)
        let before = inode("s1")
        markStopped(sessionIDs: ["s1"], root: root)
        XCTAssertNotEqual(inode("s1"), before, "replaced by rename, not rewritten in place")
        let names = try FileManager.default.contentsOfDirectory(atPath: dir.path)
        XCTAssertEqual(names, ["s1.json"], "no .json.tmp left behind")
        var st = stat()
        stat(dir.appendingPathComponent("s1.json").path, &st)
        XCTAssertEqual(st.st_mode & 0o777, 0o600, "same mode as the CLI's session writer")
    }

    // Codex finding 2: the owner finished the run between the snapshot and
    // the write. The fresh on-disk status wins; the file is not touched.
    func testFinishedRecordOnDiskIsNotOverwritten() throws {
        for status in ["completed", "failed"] {
            let json = running.replacingOccurrences(of: #""status": "running""#, with: #""status": "\#(status)""#)
            try write("s1", json)
            let before = try Data(contentsOf: dir.appendingPathComponent("s1.json"))
            XCTAssertEqual(markStopped(sessionIDs: ["s1"], root: root), [], status)
            XCTAssertEqual(try Data(contentsOf: dir.appendingPathComponent("s1.json")), before, status)
        }
    }

    // Codex finding 4: another writer's temp file is never reused or renamed
    // away; the app writes through its own unique temp name.
    func testTempFileIsUniquePerWrite() throws {
        try write("s1", running)
        let foreign = dir.appendingPathComponent("s1.json.tmp")
        try Data("another writer mid-save".utf8).write(to: foreign)
        XCTAssertEqual(markStopped(sessionIDs: ["s1"], root: root), ["s1"])
        XCTAssertEqual(try String(contentsOf: foreign, encoding: .utf8), "another writer mid-save")
        XCTAssertEqual(try object("s1")["status"] as? String, "failed")
        let names = try FileManager.default.contentsOfDirectory(atPath: dir.path).sorted()
        XCTAssertEqual(names, ["s1.json", "s1.json.tmp"], "own temp file renamed away")
    }

    func testMissingAndCorruptFilesAreSkipped() throws {
        try write("s1", running)
        try write("bad", "{ not json")
        let written = markStopped(sessionIDs: ["gone", "bad", "s1", "../escape"], root: root)
        XCTAssertEqual(written, ["s1"])
        XCTAssertEqual(try String(contentsOf: dir.appendingPathComponent("bad.json"), encoding: .utf8), "{ not json")
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir.appendingPathComponent("gone.json").path))
    }

    func testZeroStartTimeWritesNoDuration() throws {
        try write("q", #"{"id": "q", "status": "queued", "start_time": "0001-01-01T00:00:00Z"}"#)
        markStopped(sessionIDs: ["q"], root: root)
        XCTAssertNil(try object("q")["duration"])
    }

    // MARK: - outcome → marks

    func testConfirmedTargetDropsMembersThatFinished() {
        let current = RunItem(id: "group:g", sessions: [
            Session(id: "a", status: "completed", pid: 100),
            Session(id: "b", status: "running", pid: 101),
            Session(id: "c", status: "running", pid: 102),
        ])
        XCTAssertEqual(confirmedStopTarget(current, confirmedIDs: ["a", "b"])?.sessions.map(\.id), ["b"])
        XCTAssertNil(confirmedStopTarget(current, confirmedIDs: ["a"]))
    }

    func testStopToast() {
        XCTAssertEqual(stopToast(StopOutcome(signalled: ["a"], signalledPIDs: [7])), "SIGTERM sent to pid 7")
        XCTAssertEqual(stopToast(StopOutcome(signalled: ["a", "b"], signalledPIDs: [7, 8])), "SIGTERM sent to 2 processes")
        XCTAssertEqual(stopToast(StopOutcome()), "nothing running")
        XCTAssertEqual(stopToast(StopOutcome(dead: ["a"]), marked: 1), "process already dead — marked failed")
        XCTAssertEqual(stopToast(StopOutcome(dead: ["a"])), "process already dead — left to its rival process")
        XCTAssertEqual(stopToast(StopOutcome(unverified: ["a"])), "not signalled — cannot verify process identity")
        XCTAssertEqual(
            stopToast(StopOutcome(signalled: ["a"], signalledPIDs: [7], dead: ["b", "c", "d"], unverified: ["e", "f"]), marked: 1),
            "SIGTERM sent to pid 7 · process already dead — marked failed · 2 processes already dead — left to its rival process"
                + " · 2 not signalled — cannot verify process identity"
        )
    }
}
