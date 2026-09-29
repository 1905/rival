import XCTest
@testable import RivalKit

/// Real file writes in a temp root, then the debounced snapshot. The poll is
/// set to a minute in the watcher tests so only the DispatchSource can deliver
/// the change in time.
@MainActor
final class SessionStoreTests: XCTestCase {
    var root: URL!
    var dir: URL!
    var store: SessionStore?

    override func setUp() async throws {
        root = try makeTempDir()
        dir = root.appendingPathComponent("sessions", isDirectory: true)
    }

    override func tearDown() async throws {
        store?.stop()
        store = nil
    }

    func sessionJSON(_ id: String, status: String, start: String = "2026-09-26T03:00:00Z", group: String? = nil) -> String {
        var fields = [
            #""id": "\#(id)""#, #""cli": "codex""#, #""mode": "review""#, #""model": "gpt-6-astra""#,
            #""effort": "high""#, #""status": "\#(status)""#, #""start_time": "\#(start)""#,
            #""work_dir": "/src/rival""#, #""prompt": "the full prompt""#, #""prompt_preview": "the full""#,
        ]
        if let group { fields.append(#""group_id": "\#(group)""#) }
        return "{\n  " + fields.joined(separator: ",\n  ") + "\n}\n"
    }

    /// Writes like the Go CLI: tmp file, then rename.
    func save(_ id: String, _ body: String) throws {
        let tmp = dir.appendingPathComponent(id + ".json.tmp")
        try Data(body.utf8).write(to: tmp)
        _ = try FileManager.default.replaceItemAt(dir.appendingPathComponent(id + ".json"), withItemAt: tmp)
    }

    func makeStore(poll: Duration = .seconds(60)) -> SessionStore {
        let s = SessionStore(root: root, debounce: .milliseconds(250), pollInterval: poll)
        store = s
        return s
    }

    func waitUntil(_ what: String, timeout: TimeInterval = 5, _ cond: @escaping @MainActor () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !cond() {
            if Date() > deadline {
                XCTFail("timed out waiting for: \(what)")
                return
            }
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    func statuses(_ s: SessionStore) -> [String: String] {
        Dictionary(uniqueKeysWithValues: s.runs.flatMap(\.sessions).map { ($0.id, $0.status) })
    }

    func testInitialLoadGroupsFixtures() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        // The CLI names each file <id>.json.
        for name in try fixtureNames() {
            let id = try loadFixture(name).id
            try FileManager.default.copyItem(at: fixturesDir.appendingPathComponent(name),
                                             to: dir.appendingPathComponent(id + ".json"))
        }
        let s = makeStore()
        s.start()
        try await waitUntil("initial snapshot") { s.revision >= 1 }
        XCTAssertTrue(s.directoryExists)
        XCTAssertEqual(s.runs.flatMap(\.sessions).count, try fixtureNames().count)
        XCTAssertEqual(s.runs.count, s.runs.flatMap(\.sessions).count - 2, "three megareview members form one run")
        // Newest first.
        XCTAssertEqual(s.runs.first?.id, "solo:a1b2c3d4-0002-4000-8000-000000000002")
        let mega = try XCTUnwrap(s.runs.first { $0.id == "group:8a138d95-3176-4afb-aad1-a59e12b879c8" })
        XCTAssertEqual(mega.sessions.last?.mode, "consilium")
        // Snapshots drop the prompt; fullSession reads it.
        XCTAssertTrue(s.runs.flatMap(\.sessions).allSatisfy { $0.prompt == nil })
        let full = await s.fullSession(id: "a1b2c3d4-0001-4000-8000-000000000001")
        XCTAssertEqual(full?.prompt, "Review the fingerprint re-key in internal/core.")
        // Header counts come with the snapshot.
        XCTAssertEqual(s.stats, sessionStats(s.runs))
        XCTAssertEqual(s.stats.total, try fixtureNames().count)
    }

    func testWriteRewriteDeleteCorruptAndTmp() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let s = makeStore()
        s.start()
        try await waitUntil("empty snapshot") { s.revision >= 1 }
        XCTAssertTrue(s.runs.isEmpty)

        // Write.
        try save("one", sessionJSON("one", status: "running"))
        try await waitUntil("new file") { s.runs.flatMap(\.sessions).count == 1 }
        XCTAssertEqual(s.runs.map(\.id), ["solo:one"])

        // Rewrite.
        try save("one", sessionJSON("one", status: "completed"))
        try await waitUntil("rewrite") { self.statuses(s)["one"] == "completed" }

        // Temp files (fixed and unique names) are never read.
        try Data(sessionJSON("tmp", status: "running").utf8).write(to: dir.appendingPathComponent("tmp.json.tmp"))
        try Data(sessionJSON("tmp2", status: "running").utf8)
            .write(to: dir.appendingPathComponent("tmp2.json.tmp-123456"))
        // A corrupt file is skipped; the good ones stay.
        try Data("{not json".utf8).write(to: dir.appendingPathComponent("bad.json"))
        try save("two", sessionJSON("two", status: "queued", start: "2026-09-26T04:00:00Z"))
        try await waitUntil("second file") { s.runs.flatMap(\.sessions).count == 2 }
        XCTAssertEqual(s.runs.flatMap(\.sessions).map(\.id), ["two", "one"])

        // Fixing the corrupt file makes it appear on a later scan.
        try save("bad", sessionJSON("bad", status: "failed"))
        try await waitUntil("repaired file") { self.statuses(s)["bad"] == "failed" }
        XCTAssertNil(statuses(s)["tmp"])
        XCTAssertNil(statuses(s)["tmp2"])

        // Delete.
        try FileManager.default.removeItem(at: dir.appendingPathComponent("one.json"))
        try await waitUntil("deletion") { self.statuses(s)["one"] == nil }
        XCTAssertEqual(Set(s.runs.flatMap(\.sessions).map(\.id)), ["two", "bad"])
    }

    func testCorruptRewriteKeepsLastGoodCopy() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try save("one", sessionJSON("one", status: "running"))
        let s = makeStore(poll: .milliseconds(100))
        s.start()
        try await waitUntil("initial") { s.runs.flatMap(\.sessions).count == 1 }
        try Data("{\"id\": \"one\", \"stat".utf8).write(to: dir.appendingPathComponent("one.json"))
        try await Task.sleep(for: .milliseconds(600))
        XCTAssertEqual(statuses(s)["one"], "running", "a partial write must not drop the run")
        try save("one", sessionJSON("one", status: "completed"))
        try await waitUntil("recovered") { self.statuses(s)["one"] == "completed" }
    }

    func testGroupMembersArriveIntoOneRun() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let s = makeStore()
        s.start()
        try await waitUntil("empty snapshot") { s.revision >= 1 }
        try save("r1", sessionJSON("r1", status: "running", group: "g1"))
        try save("r2", sessionJSON("r2", status: "queued", group: "g1"))
        try await waitUntil("both members") { s.runs.flatMap(\.sessions).count == 2 }
        XCTAssertEqual(s.runs.map(\.id), ["group:g1"])
        XCTAssertEqual(runStatus(s.runs[0]), .running)
    }

    func testBurstIsDebouncedIntoFewSnapshots() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let s = makeStore()
        s.start()
        try await waitUntil("empty snapshot") { s.revision >= 1 }
        let before = s.revision
        for i in 0..<10 { try save("b\(i)", sessionJSON("b\(i)", status: "running")) }
        try await waitUntil("burst") { s.runs.flatMap(\.sessions).count == 10 }
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertLessThanOrEqual(s.revision - before, 2, "ten writes inside one debounce window")
    }

    func testDebounceDelaysTheSnapshot() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let s = makeStore()
        s.start()
        try await waitUntil("empty snapshot") { s.revision >= 1 }
        try save("late", sessionJSON("late", status: "running"))
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertTrue(s.runs.flatMap(\.sessions).isEmpty, "no snapshot before the 250 ms debounce")
        try await waitUntil("after debounce") { s.runs.flatMap(\.sessions).count == 1 }
    }

    func testMissingSessionsDirIsWatchedUntilItAppears() async throws {
        // root exists, root/sessions does not: the watcher sits on root.
        let s = makeStore()
        s.start()
        try await waitUntil("empty snapshot") { s.revision >= 1 }
        XCTAssertFalse(s.directoryExists)
        XCTAssertTrue(s.runs.isEmpty)

        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try await waitUntil("dir appears") { s.directoryExists }
        try save("first", sessionJSON("first", status: "running"))
        try await waitUntil("first session") { s.runs.flatMap(\.sessions).count == 1 }
    }

    func testMissingRootIsPickedUpByThePoll() async throws {
        root = root.appendingPathComponent("not-yet", isDirectory: true)
        dir = root.appendingPathComponent("sessions", isDirectory: true)
        let s = makeStore(poll: .milliseconds(200))
        s.start()
        try await waitUntil("empty snapshot") { s.revision >= 1 }
        XCTAssertFalse(s.directoryExists)

        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try save("first", sessionJSON("first", status: "running"))
        try await waitUntil("first session") { s.runs.flatMap(\.sessions).count == 1 }
        XCTAssertTrue(s.directoryExists)
    }

    func testStopFreezesTheSnapshot() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let s = makeStore(poll: .milliseconds(100))
        s.start()
        try await waitUntil("empty snapshot") { s.revision >= 1 }
        s.stop()
        try save("after", sessionJSON("after", status: "running"))
        try await Task.sleep(for: .milliseconds(500))
        XCTAssertTrue(s.runs.flatMap(\.sessions).isEmpty)
    }

    func testRootFromEnvironment() {
        XCTAssertEqual(RivalPaths.root(environment: ["RIVAL_HOME": "/tmp/fixture-home"]).path, "/tmp/fixture-home")
        XCTAssertEqual(RivalPaths.root(environment: ["RIVAL_HOME": ""]).lastPathComponent, ".rival")
        XCTAssertEqual(RivalPaths.root(environment: [:]).lastPathComponent, ".rival")
        XCTAssertEqual(RivalPaths.sessionsDir(root: URL(fileURLWithPath: "/x")).path, "/x/sessions")
    }

    // MARK: - Startup loader

    func testIsLoadingUntilFirstSnapshotAndProgressReachesTotal() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        for i in 0..<250 {
            try save(String(format: "s%03d", i), sessionJSON(String(format: "s%03d", i), status: "completed"))
        }
        let s = makeStore()
        XCTAssertTrue(s.isLoading, "loading before start")
        XCTAssertEqual(s.loadProgress, LoadProgress(done: 0, total: 0))
        s.start()
        XCTAssertTrue(s.isLoading, "loading until the first scan publishes")
        try await waitUntil("first snapshot") { s.revision >= 1 }
        XCTAssertFalse(s.isLoading)
        XCTAssertEqual(s.runs.flatMap(\.sessions).count, 250)
        XCTAssertEqual(s.loadProgress, LoadProgress(done: 250, total: 250))
        // Late progress hops must not move it backwards.
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(s.loadProgress, LoadProgress(done: 250, total: 250))
    }

    func testScannerReportsThrottledProgress() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        for i in 0..<250 {
            try save(String(format: "s%03d", i), sessionJSON(String(format: "s%03d", i), status: "completed"))
        }
        let calls = Calls()
        let result = await SessionScanner(dir: dir).scan { done, total in calls.add(done, total) }
        XCTAssertEqual(result.sessions.count, 250)
        let got = calls.all
        XCTAssertEqual(got.last?.0, 250)
        XCTAssertTrue(got.allSatisfy { $0.1 == 250 })
        XCTAssertEqual(got.count, 3, "every 100 files plus the end: \(got)")
    }

    func testEmptyDirEndsLoadingQuickly() async throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let s = makeStore()
        s.start()
        try await waitUntil("loaded", timeout: 1) { !s.isLoading }
        XCTAssertTrue(s.directoryExists)
        XCTAssertEqual(s.loadProgress, LoadProgress(done: 0, total: 0))
    }

    func testMissingDirEndsLoadingQuickly() async throws {
        let s = makeStore()
        s.start()
        try await waitUntil("loaded", timeout: 1) { !s.isLoading }
        XCTAssertFalse(s.directoryExists)
        XCTAssertTrue(s.runs.isEmpty)
    }
}

/// Progress calls from the scanner's worker threads.
private final class Calls: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [(Int, Int)] = []
    func add(_ d: Int, _ t: Int) { lock.lock(); items.append((d, t)); lock.unlock() }
    var all: [(Int, Int)] { lock.lock(); defer { lock.unlock() }; return items }
}
