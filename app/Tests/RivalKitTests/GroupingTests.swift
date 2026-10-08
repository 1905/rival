import XCTest
@testable import RivalKit

/// Parity with rival/internal/sessionview/group_test.go and the TUI row helpers
/// in rival/internal/dashboard/session_list_test.go + parity_test.go.
final class GroupingTests: XCTestCase {
    // TestGroupBucketsAndKeys
    func testGroupBucketsAndKeys() {
        let solo = sess("s1", "", "completed", "review", "codex", solModel, "high")
        let a = sess("a1", "grp", "completed", "plan", "codex", solModel, "xhigh")
        let b = sess("b1", "grp", "completed", "plan", "claude", claudeModel, "xhigh")
        let runs = groupRuns([solo, a, b])
        XCTAssertEqual(runs.map(\.id), ["solo:s1", "group:grp"])
        XCTAssertEqual(runs[1].sessions.count, 2)
        XCTAssertFalse(runs[0].isGroup)
        XCTAssertTrue(runs[1].isGroup)
    }

    // TestGroupPreservesFirstAppearanceOrder
    func testGroupPreservesFirstAppearanceOrder() {
        let first = sess("x", "g2", "completed", "review", "codex", solModel, "high")
        let second = sess("y", "g1", "completed", "review", "codex", solModel, "high")
        let third = sess("z", "g2", "completed", "review", "claude", claudeModel, "high")
        XCTAssertEqual(groupRuns([first, second, third]).map(\.id), ["group:g2", "group:g1"])
    }

    // TestPairedPlanGroupShowsRawModelsAndPlanKind: newest-first input, the
    // group restores requested order from QueuedAt.
    func testPairedPlanGroupRestoresRequestedOrder() {
        let created = Date(timeIntervalSince1970: 1_790_000_000)
        let later = created.addingTimeInterval(0.001)
        let runs = groupRuns([
            Session(id: "b", groupID: "paired", cli: "claude", mode: "plan", model: claudeModel, queuedAt: later),
            Session(id: "a", groupID: "paired", cli: "codex", mode: "plan", model: solModel, queuedAt: created),
        ])
        XCTAssertEqual(runs.count, 1)
        XCTAssertTrue(runs[0].isGroup)
        XCTAssertEqual(runKind(runs[0]), "plan")
        XCTAssertEqual(runModelName(runs[0]), solModel + " +1")
    }

    // TestSingletonClaudePlanRemainsLogicalPlanGroup
    func testSingletonPlanIsStillAGroup() {
        let runs = groupRuns([Session(id: "claude", groupID: "degraded-plan", cli: "claude", mode: "plan",
                                      model: claudeModel, status: "running")])
        XCTAssertEqual(runs.count, 1)
        XCTAssertTrue(runs[0].isGroup)
        XCTAssertEqual(runKind(runs[0]), "plan")
        XCTAssertEqual(runModelName(runs[0]), claudeModel)
    }

    func testJudgeSortsLastAndModelRankBreaksTies() {
        let judge = Session(id: "j", groupID: "g", cli: "codex", mode: "consilium", model: "gpt-5.5")
        let grok = Session(id: "r3", groupID: "g", cli: "grok", mode: "megareview", model: "grok-4.6")
        let claude = Session(id: "r2", groupID: "g", cli: "claude", mode: "megareview", model: claudeModel)
        let sol = Session(id: "r1", groupID: "g", cli: "codex", mode: "megareview", model: solModel)
        let runs = groupRuns([judge, grok, claude, sol])
        XCTAssertEqual(runs[0].sessions.map(\.id), ["r1", "r2", "r3", "j"])
    }

    func testMegareviewFixtureGroup() throws {
        let sessions = try ["mega-judge.json", "mega-reviewer-b.json", "mega-reviewer-a.json", "sol-history.json"]
            .map(loadFixture)
        let runs = groupRuns(sessions)
        XCTAssertEqual(runs.map(\.id), ["group:8a138d95-3176-4afb-aad1-a59e12b879c8", "solo:00080ac4-80be-4646-a2cc-44110d1acdf4"])
        let mega = runs[0]
        XCTAssertEqual(mega.sessions.map(\.mode), ["megareview", "megareview", "consilium"])
        XCTAssertEqual(mega.sessions.first?.model, "gpt-5.5")
        XCTAssertEqual(runKind(mega), "mega")
        XCTAssertEqual(runStatus(mega), .completed)
        XCTAssertEqual(runModelName(mega), "gpt-5.5 +1")
        XCTAssertEqual(runEffort(mega), "xhigh")
        // 14:19:59.900 (first queue) → 14:29:17.717611 (judge end) = 557.8s.
        XCTAssertEqual(runElapsed(mega, now: Date()), "9m18s")

        let sol = runs[1]
        XCTAssertEqual(runKind(sol), "raw")
        XCTAssertEqual(runModelName(sol), "gpt-5.6-sol")
        XCTAssertEqual(runElapsed(sol, now: Date()), "14m38s")
    }

    func testGroupDoesNotReorderInputArray() {
        let a = sess("a", "g", "completed", "plan", "codex", solModel, "high")
        let b = sess("b", "g", "running", "plan", "claude", claudeModel, "low")
        let input = [b, a]
        _ = groupRuns(input)
        XCTAssertEqual(input.map(\.id), ["b", "a"])
    }

    // TestStatusTier
    func testStatusTier() {
        let cases: [([String], RunStatus)] = [
            (["completed", "running", "queued"], .running),
            (["failed", "queued", "completed"], .queued),
            (["completed", "failed"], .failed),
            (["completed", "completed"], .completed),
        ]
        for (statuses, want) in cases {
            let members = statuses.enumerated().map { i, st in
                sess(String(UnicodeScalar(UInt8(97 + i))), "g", st, "review", "codex", solModel, "high")
            }
            XCTAssertEqual(runStatus(RunItem(id: "group:g", sessions: members)), want, "\(statuses)")
        }
    }

    func testSoloStatusIsItsOwn() {
        XCTAssertEqual(runStatus(solo(Session(id: "a", status: "failed"))), .failed)
        XCTAssertEqual(runStatus(solo(Session(id: "a", status: "killed"))), .unknown)
    }

    // TestKindPrecedence (group side, via the TUI short labels)
    func testKindPrecedence() {
        let cases: [([String], String)] = [
            (["plan", "plan"], "plan"),
            (["review", "review"], "mega"),
            (["raw"], "mega"),
            (["security", "plan"], "sec"),
        ]
        for (modes, want) in cases {
            let members = modes.enumerated().map { i, m in
                sess(String(UnicodeScalar(UInt8(97 + i))), "g", "completed", m, "codex", solModel, "high")
            }
            XCTAssertEqual(runKind(RunItem(id: "group:g", sessions: members)), want, "\(modes)")
        }
    }

    // TestKindLabel
    func testKindLabel() {
        func s(_ cli: String, _ mode: String) -> RunItem { solo(Session(id: "x", cli: cli, mode: mode)) }
        func g(_ modes: String...) -> RunItem {
            RunItem(id: "group:g", sessions: modes.enumerated().map { Session(id: "\($0.offset)", groupID: "g", mode: $0.element) })
        }
        let cases: [(String, RunItem, String)] = [
            ("review", s("codex", "review"), "review"),
            ("plan", s("codex", "plan"), "plan"),
            ("security", s("opencode", "security"), "sec"),
            ("raw", s("opencode", "raw"), "raw"),
            ("native", s("claude", "native"), "review"),
            ("empty mode", s("codex", ""), "review"),
            ("docker claude", s("claude", "docker"), "review/dk"),
            ("docker fable", s("fable", "docker"), "review/dk"),
            ("megareview group", g("megareview", "megareview", "consilium"), "mega"),
            ("plan group", g("plan", "plan"), "plan"),
            ("security group", g("security"), "sec"),
        ]
        for (name, item, want) in cases {
            XCTAssertEqual(runKind(item), want, name)
        }
    }

    // TestRowLabelsForEveryCLI
    func testRowLabelsForEveryCLI() {
        for s in [
            Session(id: "1", cli: "codex", mode: "review", model: solModel),
            Session(id: "2", cli: "opencode", mode: "review", model: "moonshotai/kimi-k3"),
            Session(id: "3", cli: "claude", mode: "review", model: claudeModel),
            Session(id: "4", cli: "grok", mode: "review", model: "grok-4.6"),
        ] {
            XCTAssertEqual(runModelName(solo(s)), s.model)
            XCTAssertEqual(runKind(solo(s)), "review")
        }
    }

    // TestModelNameShowsTheRawID
    func testModelName() {
        XCTAssertEqual(modelName(Session(id: "a", cli: "codex", model: "gpt-6-astra")), "gpt-6-astra")
        XCTAssertEqual(modelName(Session(id: "a", cli: "claude", model: "claude-fable-5")), "claude-fable-5")
        XCTAssertEqual(modelName(Session(id: "a", cli: "codex", model: "gpt-5.5")), "gpt-5.5")
        XCTAssertEqual(modelName(Session(id: "a", cli: "codex", model: "")), "codex")
    }

    // TestGroupModelName
    func testGroupModelName() {
        XCTAssertEqual(runModelName(group([
            Session(id: "1", groupID: "g", mode: "megareview", model: "gpt-5.5"),
            Session(id: "2", groupID: "g", mode: "megareview", model: "gemini-3.1"),
            Session(id: "3", groupID: "g", mode: "consilium", model: "kimi-k3"),
        ])), "gpt-5.5 +2")
        XCTAssertEqual(runModelName(group([
            Session(id: "1", groupID: "g", mode: "megareview", model: "gpt-6-astra"),
            Session(id: "2", groupID: "g", mode: "consilium", model: "gpt-6-astra"),
        ])), "gpt-6-astra")
        XCTAssertEqual(runModelName(solo(Session(id: "1", model: "claude-opus-5-5"))), "claude-opus-5-5")
    }

    // TestEffort + TestGroupEffortShowsMixedDefaults
    func testEffort() {
        XCTAssertEqual(runEffort(group([Session(id: "a", effort: "xhigh"), Session(id: "b", effort: "xhigh")])), "xhigh")
        XCTAssertEqual(runEffort(group([Session(id: "a", effort: "xhigh"), Session(id: "b", effort: "low")])), "mixed")
        XCTAssertEqual(runEffort(group([Session(id: "a", effort: "ultra"), Session(id: "b", effort: "low")])), "mixed")
        XCTAssertEqual(runEffort(RunItem(id: "group:g", sessions: [])), "")
    }

    // TestProjectName
    func testProjectName() {
        XCTAssertEqual(projectName("/a/b/orbit-web"), "orbit-web")
        XCTAssertEqual(projectName("/a/b/orbit-web/"), "orbit-web")
        XCTAssertEqual(projectName(""), "-")
        XCTAssertEqual(projectName("/"), "/")
        XCTAssertEqual(projectName("rel"), "rel")
    }

    // TestStatusGlyph
    func testStatusGlyph() {
        let cases: [(String, String)] = [("running", "⠋"), ("queued", "◌"), ("completed", "✓"),
                                         ("failed", "✗"), ("killed", "·"), ("", "·")]
        for (status, want) in cases {
            XCTAssertEqual(statusGlyph(runStatus(solo(Session(id: "a", status: status))), spin: "⠋"), want, status)
        }
    }

    // TestTUIRowValuesMatchSharedDerivations
    func testPlanGroupRowValues() {
        let base = Date().addingTimeInterval(-20 * 60)
        let firstEnd = base.addingTimeInterval(4 * 60)
        let secondEnd = firstEnd.addingTimeInterval(3 * 60)
        let item = group([
            Session(id: "a", groupID: "g", cli: "codex", mode: "plan", model: solModel, effort: "xhigh",
                    status: "completed", startTime: base, endTime: firstEnd),
            Session(id: "b", groupID: "g", cli: "claude", mode: "plan", model: claudeModel, effort: "xhigh",
                    status: "completed", startTime: firstEnd, endTime: secondEnd),
        ])
        XCTAssertEqual(runStatus(item), .completed)
        XCTAssertEqual(runEffort(item), "xhigh")
        XCTAssertEqual(runKind(item), "plan")
        XCTAssertEqual(runModelName(item), solModel + " +1")
        XCTAssertEqual(runElapsed(item, now: Date()), "7m0s")
    }
}

/// sessionview.Elapsed parity.
final class ElapsedTests: XCTestCase {
    let now = Date(timeIntervalSince1970: 1_790_400_000)

    // TestElapsedSpansTheWholeGroup
    func testElapsedSpansTheWholeGroup() {
        let base = now.addingTimeInterval(-30 * 60)
        let sequential = group([
            Session(id: "a", status: "completed", startTime: base, endTime: base.addingTimeInterval(240)),
            Session(id: "b", status: "completed", startTime: base.addingTimeInterval(240), endTime: base.addingTimeInterval(420)),
        ])
        XCTAssertEqual(runElapsed(sequential, now: now), "7m0s")

        let overlapping = group([
            Session(id: "a", status: "completed", startTime: base, endTime: base.addingTimeInterval(600)),
            Session(id: "b", status: "completed", startTime: base.addingTimeInterval(120), endTime: base.addingTimeInterval(300)),
        ])
        XCTAssertEqual(runElapsed(overlapping, now: now), "10m0s")
    }

    // TestElapsedUsesDurationFallbackAndQueuedAt
    func testDurationFallbackAndQueuedAt() {
        let base = now.addingTimeInterval(-20 * 60)
        XCTAssertEqual(runElapsed(solo(Session(id: "a", status: "completed", startTime: base, duration: "3m0s")), now: now), "3m0s")
        let queued = solo(Session(id: "a", status: "queued", queuedAt: now.addingTimeInterval(-600)))
        XCTAssertEqual(runElapsed(queued, now: now), "10m0s")
    }

    // TestElapsedWithoutStartIsDash
    func testWithoutStartIsDash() {
        XCTAssertEqual(runElapsed(solo(Session(id: "a", status: "queued")), now: now), "-")
        XCTAssertEqual(runElapsed(solo(Session(id: "a", status: "failed", startTime: now)), now: now), "-")
    }

    func testRunningExtendsToNowAndRounds() {
        let running = solo(Session(id: "a", status: "running", startTime: now.addingTimeInterval(-3723.6)))
        XCTAssertEqual(runElapsed(running, now: now), "1h2m4s")
        let short = solo(Session(id: "a", status: "running", startTime: now.addingTimeInterval(-0.4)))
        XCTAssertEqual(runElapsed(short, now: now), "0s")
    }

    func testQueuedSoloTimeLabel() {
        let q = solo(Session(id: "a", status: "queued", queuedAt: now.addingTimeInterval(-65), queuePosition: 2))
        XCTAssertEqual(runTimeLabel(q, now: now), "#2 1m5s")
    }

    func testGoDurationParseAndFormat() {
        XCTAssertEqual(parseGoDuration("1m23s"), 83)
        XCTAssertEqual(parseGoDuration("1h2m3s"), 3723)
        XCTAssertEqual(parseGoDuration("1.5h"), 5400)
        XCTAssertEqual(parseGoDuration("0"), 0)
        XCTAssertEqual(parseGoDuration("-2s"), -2)
        XCTAssertEqual(parseGoDuration("300ms")!, 0.3, accuracy: 1e-9)
        XCTAssertEqual(parseGoDuration("5µs")!, 5e-6, accuracy: 1e-12)
        XCTAssertNil(parseGoDuration(""))
        XCTAssertNil(parseGoDuration("3"))
        XCTAssertNil(parseGoDuration("3x"))
        XCTAssertEqual(formatGoDuration(seconds: 0), "0s")
        XCTAssertEqual(formatGoDuration(seconds: 45), "45s")
        XCTAssertEqual(formatGoDuration(seconds: 420), "7m0s")
        XCTAssertEqual(formatGoDuration(seconds: 3600), "1h0m0s")
    }
}
