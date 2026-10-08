import Darwin
import XCTest
@testable import RivalKit

final class FakeInspector: ProcessInspector {
    var starts: [Int32: Int64]
    init(_ starts: [Int32: Int64]) { self.starts = starts }
    func startTime(pid: Int32) -> Int64? { starts[pid] }
}

final class FakeSignaller: Signaller {
    var sent: [Int32] = []
    var failing: Set<Int32> = []
    func terminate(pid: Int32) -> Bool {
        if failing.contains(pid) { return false }
        sent.append(pid)
        return true
    }
}

/// The PID guard (TUI liveTargets/stopSessions, b497d86). Fakes only; no real
/// signal is ever sent.
final class StopTests: XCTestCase {
    func run(_ s: Session...) -> RunItem { RunItem(id: "group:g", sessions: s) }

    func testMatchingStartTimeIsSignalled() {
        let sig = FakeSignaller()
        let item = run(Session(id: "a", status: "running", pid: 100, pidStart: 5))
        XCTAssertEqual(stop(item, inspector: FakeInspector([100: 5]), signaller: sig),
                       StopOutcome(signalled: ["a"], signalledPIDs: [100]))
        XCTAssertEqual(sig.sent, [100])
    }

    func testStartTimeMismatchMeansNoSignal() {
        let sig = FakeSignaller()
        let item = run(Session(id: "a", status: "running", pid: 100, pidStart: 5))
        XCTAssertEqual(stop(item, inspector: FakeInspector([100: 6]), signaller: sig), StopOutcome(dead: ["a"]))
        XCTAssertEqual(sig.sent, [])
    }

    func testDeadProcessMeansNoSignal() {
        let sig = FakeSignaller()
        let item = run(Session(id: "a", status: "running", pid: 100, pidStart: 5))
        XCTAssertEqual(stop(item, inspector: FakeInspector([:]), signaller: sig), StopOutcome(dead: ["a"]))
        XCTAssertEqual(sig.sent, [])
    }

    // Codex finding 1: a live PID without a recorded start time could be any
    // process that reused the PID. It is never signalled.
    func testNoRecordedStartIsNeverSignalled() {
        let sig = FakeSignaller()
        let item = run(
            Session(id: "a", status: "queued", pid: 100),
            Session(id: "zero", status: "running", pid: 101, pidStart: 0)
        )
        let inspector = FakeInspector([100: 999, 101: 5])
        XCTAssertEqual(stopTargetState(item.sessions[0], inspector: inspector), .unverified)
        XCTAssertEqual(stopTargetState(item.sessions[1], inspector: inspector), .unverified)
        XCTAssertEqual(stop(item, inspector: inspector, signaller: sig), StopOutcome(unverified: ["a", "zero"]))
        XCTAssertEqual(sig.sent, [])
    }

    func testNoRecordedStartAndGonePIDIsDead() {
        let sig = FakeSignaller()
        let item = run(Session(id: "a", status: "running", pid: 100))
        XCTAssertEqual(stop(item, inspector: FakeInspector([:]), signaller: sig), StopOutcome(dead: ["a"]))
        XCTAssertEqual(sig.sent, [])
    }

    func testFinishedOrPidlessMembersAreNotTargets() {
        let sig = FakeSignaller()
        let item = run(
            Session(id: "done", status: "completed", pid: 100, pidStart: 5),
            Session(id: "failed", status: "failed", pid: 101, pidStart: 6),
            Session(id: "nopid", status: "running", pid: 0)
        )
        let inspector = FakeInspector([100: 5, 101: 6])
        XCTAssertTrue(stop(item, inspector: inspector, signaller: sig).nothingRunning)
        XCTAssertEqual(stopCandidates(item), [])
        XCTAssertEqual(sig.sent, [])
    }

    func testMixedGroupSignalsOnlyTheLiveMatch() {
        let sig = FakeSignaller()
        let item = run(
            Session(id: "live", status: "running", pid: 100, pidStart: 5),
            Session(id: "reused", status: "running", pid: 101, pidStart: 6),
            Session(id: "judge", status: "queued", pid: 102, pidStart: 7)
        )
        let inspector = FakeInspector([100: 5, 101: 60, 102: 7])
        XCTAssertEqual(stopCandidates(item).map(\.id), ["live", "reused", "judge"])
        XCTAssertEqual(stop(item, inspector: inspector, signaller: sig),
                       StopOutcome(signalled: ["live", "judge"], signalledPIDs: [100, 102], dead: ["reused"]))
        XCTAssertEqual(sig.sent, [100, 102])
    }

    func testUndeliverableSignalCountsAsDead() {
        let sig = FakeSignaller()
        sig.failing = [100]
        let item = run(Session(id: "a", status: "running", pid: 100, pidStart: 5))
        XCTAssertEqual(stop(item, inspector: FakeInspector([100: 5]), signaller: sig), StopOutcome(dead: ["a"]))
    }

    // Codex finding 4: the app finalizes a dead member only when its owning
    // rival process is gone too (the CLI's reaper rule). A live owner finishes its own run.
    func testDeadToMarkFollowsTheOwnerRule() {
        let item = run(
            Session(id: "legacy", status: "running", pid: 100, pidStart: 5),
            Session(id: "ownerLive", status: "running", pid: 101, pidStart: 6, ownerPID: 200, ownerPIDStart: 20),
            Session(id: "ownerGone", status: "running", pid: 102, pidStart: 7, ownerPID: 201, ownerPIDStart: 21),
            Session(id: "ownerReused", status: "running", pid: 103, pidStart: 8, ownerPID: 202, ownerPIDStart: 22),
            Session(id: "ownerNoStart", status: "running", pid: 104, pidStart: 9, ownerPID: 203),
            // Signalled with no owner recorded: still never marked by the app.
            Session(id: "signalled", status: "running", pid: 105, pidStart: 10)
        )
        let inspector = FakeInspector([200: 20, 202: 99, 203: 1, 105: 10])
        let outcome = stop(item, inspector: inspector, signaller: FakeSignaller())
        XCTAssertEqual(outcome.signalled, ["signalled"])
        XCTAssertEqual(outcome.dead, ["legacy", "ownerLive", "ownerGone", "ownerReused", "ownerNoStart"])
        XCTAssertEqual(deadToMark(item, outcome: outcome, inspector: inspector), ["legacy", "ownerGone", "ownerReused"])
    }

    func testOwnerFieldsDecode() throws {
        let json = #"{"id":"a","owner_pid":43897,"owner_pid_start":1790392198373568000}"#
        let s = try JSONDecoder().decode(Session.self, from: Data(json.utf8))
        XCTAssertEqual(s.ownerPID, 43897)
        XCTAssertEqual(s.ownerPIDStart, 1_790_392_198_373_568_000)
        let bare = try JSONDecoder().decode(Session.self, from: Data(#"{"id":"b","owner_pid":0}"#.utf8))
        XCTAssertNil(bare.ownerPID)
    }

    // The whole confirmed stop with fakes and a temp root: the snapshot is
    // re-read, only the matching PID is signalled, only a dead member whose
    // owner is gone is written, and the toast sums it up.
    func testPerformStop() throws {
        let root = try makeTempDir()
        let dir = RivalPaths.sessionsDir(root: root)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        for id in ["live", "dead", "ownedDead", "finished"] {
            try Data(#"{"id":"\#(id)","status":"running"}"#.utf8).write(to: dir.appendingPathComponent(id + ".json"))
        }
        let item = run(
            Session(id: "live", status: "running", pid: 100, pidStart: 5),
            Session(id: "dead", status: "running", pid: 101, pidStart: 6),
            Session(id: "ownedDead", status: "running", pid: 102, pidStart: 7, ownerPID: 200, ownerPIDStart: 20),
            // Finished after the sheet opened: dropped, never signalled.
            Session(id: "finished", status: "completed", pid: 103, pidStart: 8)
        )
        let sig = FakeSignaller()
        let toast = performStop(
            runID: item.id, confirmedIDs: ["live", "dead", "ownedDead", "finished"], runs: [item], root: root,
            inspector: FakeInspector([100: 5, 103: 8, 200: 20]), signaller: sig
        )
        XCTAssertEqual(sig.sent, [100])
        XCTAssertEqual(toast, "SIGTERM sent to pid 100 · process already dead — marked failed"
            + " · process already dead — left to its rival process")
        func status(_ id: String) throws -> String? {
            let data = try Data(contentsOf: dir.appendingPathComponent(id + ".json"))
            return (try JSONSerialization.jsonObject(with: data) as? [String: Any])?["status"] as? String
        }
        XCTAssertEqual(try status("dead"), "failed")
        XCTAssertEqual(try status("live"), "running", "the owner finalizes a signalled run")
        XCTAssertEqual(try status("ownedDead"), "running", "a live owner finalizes its own run")
        XCTAssertEqual(try status("finished"), "running", "not a stop target")

        XCTAssertEqual(performStop(runID: "group:gone", confirmedIDs: ["live"], runs: [item], root: root,
                                   inspector: FakeInspector([:]), signaller: sig), "nothing running")
        XCTAssertEqual(sig.sent, [100])
    }

    // The real inspector reads this test process only; it sends nothing.
    func testSystemInspectorUsesUnixNanoseconds() throws {
        let inspector = SystemProcessInspector()
        let start = try XCTUnwrap(inspector.startTime(pid: getpid()))
        let startDate = Date(timeIntervalSince1970: Double(start) / 1e9)
        XCTAssertLessThanOrEqual(startDate, Date())
        XCTAssertGreaterThan(startDate, Date().addingTimeInterval(-24 * 3600))
        XCTAssertEqual(start % 1000, 0, "microsecond source, scaled to ns like the CLI")
        XCTAssertEqual(inspector.startTime(pid: getpid()), start, "stable across reads")
        XCTAssertNil(inspector.startTime(pid: 1 << 24))
        XCTAssertNil(inspector.startTime(pid: 0))
        XCTAssertEqual(stopTargetState(Session(id: "me", pid: getpid(), pidStart: start), inspector: inspector), .signal)
        XCTAssertEqual(stopTargetState(Session(id: "me", pid: getpid(), pidStart: start + 1), inspector: inspector), .gone)
    }
}
