import XCTest
@testable import RivalKit

final class RunDetailModelTests: XCTestCase {
    let a = Session(id: "a", mode: "megareview", model: "gpt-6-astra", status: "running")
    let b = Session(id: "b", mode: "megareview", model: "claude-opus-5-5", status: "running")
    let judge = Session(id: "j", mode: "consilium", model: "gpt-5.5", status: "queued")

    func testOpeningARunResets() {
        var m = RunDetailModel()
        let g = RunItem(id: "group:g", sessions: [a, b])
        m.sync(g)
        m.selectMember(id: "b", in: g)
        m.tab = .info
        m.userScrolled(atBottom: false)

        m.sync(RunItem(id: "solo:x", sessions: [Session(id: "x", status: "completed")]))
        XCTAssertEqual(m.runID, "solo:x")
        XCTAssertEqual(m.memberID, "x")
        XCTAssertEqual(m.tab, .result)
        XCTAssertTrue(m.follow)
    }

    func testTabOrder() {
        XCTAssertEqual(DetailTab.allCases.map(\.rawValue), ["Result", "Raw", "Prompt", "Info"])
    }

    func testOpenFinishedRunOnResult() {
        var m = RunDetailModel()
        m.sync(solo(Session(id: "f", status: "failed")))
        XCTAssertEqual(m.tab, .result)
    }

    func testOpenLiveRunOnRawWithFollow() {
        for status in ["running", "queued"] {
            var m = RunDetailModel()
            m.sync(solo(Session(id: "l", status: status)))
            XCTAssertEqual(m.tab, .raw, status)
            XCTAssertTrue(m.follow)
        }
    }

    func testLiveRunFinishingOnRawWithFollowMovesToResult() {
        var m = RunDetailModel()
        m.sync(solo(Session(id: "l", status: "running")))
        m.sync(solo(Session(id: "l", status: "running")))
        XCTAssertEqual(m.tab, .raw, "a refresh while live changes nothing")
        m.sync(solo(Session(id: "l", status: "completed")))
        XCTAssertEqual(m.tab, .result)
        m.tab = .raw
        m.sync(solo(Session(id: "l", status: "completed")))
        XCTAssertEqual(m.tab, .raw, "only the live→finished edge switches")
    }

    func testLiveRunFinishingScrolledUpStays() {
        var m = RunDetailModel()
        m.sync(solo(Session(id: "l", status: "running")))
        m.userScrolled(atBottom: false)
        m.sync(solo(Session(id: "l", status: "completed")))
        XCTAssertEqual(m.tab, .raw)
    }

    func testLiveRunFinishingOnOtherTabStays() {
        var m = RunDetailModel()
        m.sync(solo(Session(id: "l", status: "running")))
        m.tab = .prompt
        m.sync(solo(Session(id: "l", status: "completed")))
        XCTAssertEqual(m.tab, .prompt)
    }

    func testMemberSwitchIsNotAFinish() {
        // The user leaves a live member for a finished one: no switch.
        let done = Session(id: "b", mode: "megareview", status: "completed")
        var m = RunDetailModel()
        m.sync(group([a, done]))
        XCTAssertEqual(m.tab, .raw)
        m.selectMember(id: "b", in: group([a, done]))
        XCTAssertEqual(m.tab, .raw)
        // The picked member itself finishing does switch.
        m.selectMember(id: "a", in: group([a, done]))
        let finished = Session(id: "a", mode: "megareview", model: "gpt-6-astra", status: "completed")
        m.sync(group([finished, done]))
        XCTAssertEqual(m.tab, .result)
    }

    func testSelectMemberTracksWithoutSync() {
        // Picking a live member tracks it, so its own finish switches to
        // Result on the next refresh with no extra `sync` in between.
        let done = Session(id: "a", mode: "megareview", status: "completed")
        var m = RunDetailModel()
        m.sync(group([done, b]))
        XCTAssertEqual(m.tab, .result)
        m.tab = .raw
        m.selectMember(id: "b", in: group([done, b]))
        let finished = Session(id: "b", mode: "megareview", model: "claude-opus-5-5", status: "completed")
        m.sync(group([done, finished]))
        XCTAssertEqual(m.tab, .result)
    }

    func testMemberIsAnchoredByIDAcrossReorders() {
        var m = RunDetailModel()
        let g = RunItem(id: "group:g", sessions: [a, b])
        m.sync(g)
        m.selectMember(id: "b", in: g)
        m.tab = .prompt
        m.userScrolled(atBottom: false)

        // A refresh re-sorts the group and adds the judge.
        let refreshed = RunItem(id: "group:g", sessions: [b, a, judge])
        m.sync(refreshed)
        XCTAssertEqual(m.member(in: refreshed)?.id, "b")
        XCTAssertEqual(m.tab, .prompt, "a refresh keeps the tab")
        XCTAssertFalse(m.follow, "a refresh keeps follow paused")
    }

    func testVanishedMemberFallsBackToFirst() {
        var m = RunDetailModel()
        let g = RunItem(id: "group:g", sessions: [a, b])
        m.sync(g)
        m.selectMember(id: "b", in: g)
        let shrunk = RunItem(id: "group:g", sessions: [a])
        XCTAssertEqual(m.member(in: shrunk)?.id, "a", "safe before sync")
        m.sync(shrunk)
        XCTAssertEqual(m.memberID, "a")
    }

    func testMemberBeforeSyncIsFirst() {
        let m = RunDetailModel()
        XCTAssertEqual(m.member(in: RunItem(id: "group:g", sessions: [a, b]))?.id, "a")
        XCTAssertNil(m.member(in: RunItem(id: "group:e", sessions: [])))
    }

    func testFollow() {
        var m = RunDetailModel()
        let g = RunItem(id: "group:g", sessions: [a, b])
        m.sync(g)
        XCTAssertTrue(m.follow)
        m.userScrolled(atBottom: false)
        XCTAssertFalse(m.follow)
        m.userScrolled(atBottom: true)
        XCTAssertTrue(m.follow, "back at the tail resumes")
        m.setFollow(false)
        m.selectMember(id: "b", in: g)
        XCTAssertTrue(m.follow, "a new member starts at its tail")
        m.userScrolled(atBottom: false)
        m.selectMember(id: "b", in: g)
        XCTAssertFalse(m.follow, "re-selecting the same member changes nothing")
        m.setFollow(true)
        XCTAssertTrue(m.follow)
    }

    func testLabelsAndIDs() {
        XCTAssertEqual(memberLabel(judge), "judge")
        XCTAssertEqual(memberLabel(a), "gpt-6-astra")
        XCTAssertEqual(memberLabel(Session(id: "c", cli: "grok")), "grok")
        let g = Session(id: "11111111-aaaa", groupID: "8a138d95-3176")
        XCTAssertEqual(runShortID(RunItem(id: "group:x", sessions: [g])), "8a138d95")
        XCTAssertEqual(runShortID(RunItem(id: "solo:y", sessions: [Session(id: "a1b2c3d4-0001")])), "a1b2c3d4")
    }

    func testInfoRowsOrderAndValues() throws {
        let s = try loadFixture("failed.json")
        let rows = infoRows(s, now: Date(), timeZone: TimeZone(identifier: "UTC")!)
        XCTAssertEqual(rows.map(\.label), [
            "id", "group id", "cli", "model", "effort", "mode", "status", "exit", "started", "ended",
            "duration", "queued at", "workdir", "scope", "account", "pid", "output", "log",
        ])
        let byLabel = Dictionary(uniqueKeysWithValues: rows.map { ($0.label, $0.value) })
        XCTAssertEqual(byLabel["id"], s.id)
        XCTAssertEqual(byLabel["group id"], "-")
        XCTAssertEqual(byLabel["status"], "failed")
        XCTAssertEqual(byLabel["output"], "\(s.outputBytes) bytes, \(s.outputLines) lines")
        XCTAssertFalse(rows.contains { $0.value.isEmpty })
    }

    func testInfoTimesAreLocalAndZeroIsDash() {
        let s = Session(id: "x", startTime: parseRFC3339("2026-09-26T03:10:00Z")!)
        let rows = infoRows(s, now: Date(), timeZone: TimeZone(secondsFromGMT: 8 * 3600)!)
        let byLabel = Dictionary(uniqueKeysWithValues: rows.map { ($0.label, $0.value) })
        XCTAssertEqual(byLabel["started"], "2026-09-26 11:10:00")
        XCTAssertEqual(byLabel["ended"], "-")
        XCTAssertEqual(infoRows(Session(id: "z"), now: Date()).first { $0.label == "started" }?.value, "-")
    }

    func testSessionStatsCountSessionsNotRows() {
        let runs = [
            RunItem(id: "group:g", sessions: [a, b, judge]),
            RunItem(id: "solo:c", sessions: [Session(id: "c", status: "completed")]),
            RunItem(id: "solo:f", sessions: [Session(id: "f", status: "failed")]),
            RunItem(id: "solo:u", sessions: [Session(id: "u", status: "weird")]),
        ]
        let st = sessionStats(runs)
        XCTAssertEqual(st.running, 2)
        XCTAssertEqual(st.queued, 1)
        XCTAssertEqual(st.completed, 1)
        XCTAssertEqual(st.failed, 1)
        XCTAssertEqual(st.total, 6)
        XCTAssertEqual(sessionStats([]), SessionStats())
    }

    func testSpinnerCyclesAtTwelveFPS() {
        let t0 = Date(timeIntervalSinceReferenceDate: 0)
        XCTAssertEqual(spinnerFrame(at: t0), "⠋")
        XCTAssertEqual(spinnerFrame(at: t0.addingTimeInterval(1.0 / 12 + 0.001)), "⠙")
        XCTAssertEqual(spinnerFrame(at: t0.addingTimeInterval(10.0 / 12 + 0.001)), "⠋", "wraps after 10 frames")
        XCTAssertEqual(spinnerFrame(at: Date(timeIntervalSinceReferenceDate: -0.05)), "⠏", "negative time stays in range")
    }

    func testLogoGradient() {
        XCTAssertEqual(blendHex(Theme.logoStopHex, t: 0), 0x8B6CD9)
        XCTAssertEqual(blendHex(Theme.logoStopHex, t: 0.5), 0x4FB8CC)
        XCTAssertEqual(blendHex(Theme.logoStopHex, t: 1), 0x6FC985)
        XCTAssertEqual(blendHex(Theme.logoStopHex, t: 7), 0x6FC985, "clamped")
        XCTAssertEqual(blendHex([0x000000, 0xFFFFFF], t: 0.5), 0x808080)
        XCTAssertEqual(blendHex([0x123456], t: 0.3), 0x123456)
        XCTAssertEqual(blendHex([], t: 0.3), 0)
    }
}
