import XCTest
@testable import RivalKit

final class FinishDetectorTests: XCTestCase {
    let t0 = Date(timeIntervalSince1970: 1_790_000_000)

    func run(_ id: String, _ status: String, model: String = "gpt-6-astra", error: String? = nil,
             start: TimeInterval = 0, end: TimeInterval? = nil) -> Session {
        Session(id: id, cli: "codex", mode: "review", model: model, effort: "high", promptPreview: "Review the re-key",
                status: status, startTime: t0.addingTimeInterval(start),
                endTime: end.map { t0.addingTimeInterval($0) }, workDir: "/src/orbit-web", error: error)
    }

    func testFirstSnapshotNeverFires() {
        var d = FinishDetector()
        XCTAssertEqual(d.update([solo(run("a", "completed")), solo(run("b", "failed"))]), [])
    }

    func testLiveToFinishedFiresOnce() {
        var d = FinishDetector()
        _ = d.update([solo(run("a", "running")), solo(run("q", "queued")), solo(run("x", "running"))])
        let next = [solo(run("a", "completed")), solo(run("q", "failed")), solo(run("x", "running"))]
        XCTAssertEqual(d.update(next).map(\.id), ["solo:a", "solo:q"])
        XCTAssertEqual(d.update(next), [], "an unchanged snapshot fires nothing")
    }

    func testRunFirstSeenFinishedNeverFires() {
        var d = FinishDetector()
        _ = d.update([solo(run("a", "running"))])
        XCTAssertEqual(d.update([solo(run("a", "running")), solo(run("new", "completed"))]), [])
    }

    func testQueuedToRunningAndFinishedToFinishedDoNotFire() {
        XCTAssertEqual(finishedRuns(previous: [solo(run("a", "queued"))], current: [solo(run("a", "running"))]), [])
        XCTAssertEqual(finishedRuns(previous: [solo(run("a", "failed"))], current: [solo(run("a", "completed"))]), [])
        XCTAssertEqual(finishedRuns(previous: [solo(run("a", "running"))], current: [solo(run("a", "weird"))]), [])
    }

    func testVanishedRunDoesNotFire() {
        XCTAssertEqual(finishedRuns(previous: [solo(run("a", "running"))], current: []), [])
    }

    func testGroupFiresWhenTheLastMemberEnds() {
        var d = FinishDetector()
        _ = d.update([group([sess("r1", "g", "running", "megareview", "codex", "m1", "high"),
                             sess("r2", "g", "running", "megareview", "codex", "m2", "high")])])
        XCTAssertEqual(d.update([group([sess("r1", "g", "completed", "megareview", "codex", "m1", "high"),
                                        sess("r2", "g", "running", "megareview", "codex", "m2", "high")])]), [])
        let done = d.update([group([sess("r1", "g", "completed", "megareview", "codex", "m1", "high"),
                                    sess("r2", "g", "failed", "megareview", "codex", "m2", "high")])])
        XCTAssertEqual(done.map(\.id), ["group:g"])
    }

    func testFinishNoteText() {
        let ok = solo(run("a", "completed", start: 0, end: 384))
        XCTAssertEqual(finishNote(ok), FinishNote(
            runID: "solo:a", title: "✓ review orbit-web · gpt-6-astra · 6m24s", body: "Review the re-key"))

        let bad = solo(run("b", "failed", error: "\n  codex exited 1: \u{1B}[31mquota\u{1B}[0m\nmore", start: 0, end: 5))
        XCTAssertEqual(finishNote(bad).title, "✗ failed review orbit-web · gpt-6-astra · 5s")
        XCTAssertEqual(finishNote(bad).body, "codex exited 1: quota")
    }

    func testMenuBarRunsSplitsLiveAndRecent() {
        let runs = [
            solo(run("live1", "running", start: 900)),
            solo(run("old", "completed", start: 0, end: 10)),
            solo(run("q", "queued", start: 800)),
            solo(run("f", "failed", start: 100, end: 700)),
            solo(run("c1", "completed", start: 100, end: 200)),
            solo(run("c2", "completed", start: 100, end: 300)),
            solo(run("c3", "completed", start: 100, end: 400)),
            solo(run("c4", "completed", start: 100, end: 500)),
            solo(run("odd", "weird")),
        ]
        let (live, recent) = menuBarRuns(runs)
        XCTAssertEqual(live.map(\.id), ["solo:live1", "solo:q"])
        XCTAssertEqual(recent.map(\.id), ["solo:f", "solo:c4", "solo:c3", "solo:c2", "solo:c1"])
        XCTAssertEqual(menuBarRuns(runs, recentLimit: 0).recent, [])
    }

    func testRunEndTimeFallsBackToDuration() {
        let s = Session(id: "d", status: "completed", startTime: t0, duration: "1m30s")
        XCTAssertEqual(runEndTime(solo(s)), t0.addingTimeInterval(90))
        let none = Session(id: "n", status: "completed", startTime: t0)
        XCTAssertEqual(runEndTime(solo(none)), t0)
    }
}
