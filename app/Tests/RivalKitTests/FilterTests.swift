import XCTest
@testable import RivalKit

/// Parity with TestSectionFor, TestMatchesFilter, TestBuildRows and
/// TestBuildRowsGroupsOutOfOrderItems in rival/internal/dashboard/session_list_test.go.
final class FilterTests: XCTestCase {
    var calendar: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Europe/Berlin")!
        return c
    }()

    func date(_ y: Int, _ m: Int, _ d: Int, _ h: Int = 0, _ min: Int = 0, _ s: Int = 0) -> Date {
        calendar.date(from: DateComponents(year: y, month: m, day: d, hour: h, minute: min, second: s))!
    }

    func days(_ n: Int, from d: Date) -> Date { calendar.date(byAdding: .day, value: n, to: d)! }

    func testSectionFor() {
        let now = date(2026, 9, 26, 11, 40)
        let cases: [(Date, DaySection)] = [
            (now.addingTimeInterval(-3600), .today),
            (date(2026, 9, 26, 0, 0, 1), .today),
            (date(2026, 9, 25, 23, 59), .yesterday),
            (days(-3, from: now), .thisWeek),
            (days(-30, from: now), .older),
            (Date.goZero, .older),
        ]
        for (t, want) in cases {
            XCTAssertEqual(section(for: t, now: now, calendar: calendar), want, "\(t)")
        }
    }

    func testSectionBoundaries() {
        let now = date(2026, 9, 26, 11, 40)
        XCTAssertEqual(section(for: date(2026, 9, 26), now: now, calendar: calendar), .today)
        XCTAssertEqual(section(for: date(2026, 9, 25), now: now, calendar: calendar), .yesterday)
        XCTAssertEqual(section(for: date(2026, 9, 20), now: now, calendar: calendar), .thisWeek)
        XCTAssertEqual(section(for: date(2026, 9, 19, 23, 59, 59), now: now, calendar: calendar), .older)
    }

    // Berlin leaves DST on 2026-10-25 (that day is 25h long). Boundaries are
    // calendar day starts, not now minus 24h.
    func testSectionAcrossDSTSwitch() {
        let now = date(2026, 10, 26, 0, 30)
        XCTAssertEqual(section(for: date(2026, 10, 25, 0, 10), now: now, calendar: calendar), .yesterday)
        XCTAssertEqual(section(for: date(2026, 10, 24, 23, 50), now: now, calendar: calendar), .thisWeek)
    }

    func filterFixture(_ now: Date) -> [RunItem] {
        [
            solo(Session(id: "aaaaaaaa-1", cli: "codex", mode: "review", model: "gpt-6-astra", effort: "xhigh",
                         promptPreview: "review the fingerprint re-key", status: "running",
                         startTime: now.addingTimeInterval(-60), workDir: "/src/orbit-web")),
            solo(Session(id: "bbbbbbbb-2", cli: "claude", mode: "plan", model: "claude-opus-5-5", effort: "medium",
                         reviewScope: "plans/2026-09-26-service-identity", status: "completed",
                         startTime: now.addingTimeInterval(-7200), workDir: "/src/ledger")),
            solo(Session(id: "cccccccc-3", cli: "codex", mode: "review", model: "gpt-5.5", effort: "high",
                         status: "failed", startTime: days(-1, from: now), workDir: "/src/orbit-web")),
            solo(Session(id: "dddddddd-4", cli: "codex", mode: "review", model: "gpt-5.5", effort: "high",
                         status: "completed", startTime: days(-40, from: now), workDir: "/src/quartz-ui")),
        ]
    }

    func testMatchesFilter() {
        let items = filterFixture(date(2026, 9, 26, 12))
        let cases: [([String], [Bool])] = [
            ([], [true, true, true, true]),
            (["orbit"], [true, false, true, false]),
            (["orbit", "failed"], [false, false, true, false]),
            (["opus"], [false, true, false, false]),
            (["fingerprint"], [true, false, false, false]),
            (["service-identity"], [false, true, false, false]),
            (["cccccccc"], [false, false, true, false]),
            (["plan"], [false, true, false, false]),
            (["xhigh"], [true, false, false, false]),
            (["nothing-matches"], [false, false, false, false]),
            // Case-insensitive: the app does not pre-lowercase terms.
            (["ORBIT", "Failed"], [false, false, true, false]),
        ]
        for (terms, want) in cases {
            XCTAssertEqual(items.map { matches($0, terms: terms) }, want, "\(terms)")
        }
    }

    func testFilterTerms() {
        XCTAssertEqual(filterTerms("  Orbit   HIGH "), ["orbit", "high"])
        XCTAssertEqual(filterTerms(""), [])
    }

    func testMatchSpansNoFieldBoundary() {
        // "high" + "\n" + "orbit": a term cannot bridge two fields.
        let item = solo(Session(id: "x", effort: "high", workDir: "/src/orbit"))
        XCTAssertFalse(matches(item, terms: ["highorbit"]))
        XCTAssertFalse(matches(item, terms: ["high orbit"]))
    }

    func summary(_ sections: [RunSection]) -> [String] {
        sections.flatMap { ["#" + $0.section.rawValue] + $0.items.map { String($0.primary!.id.prefix(1)) } }
    }

    func testBuildRows() {
        let now = date(2026, 9, 26, 12)
        let items = filterFixture(now)
        let cases: [(String, StatusTab, String, [String])] = [
            ("all, no filter", .all, "", ["#TODAY", "a", "b", "#YESTERDAY", "c", "#OLDER", "d"]),
            ("filter case-insensitive", .all, "ORBIT", ["#TODAY", "a", "#YESTERDAY", "c"]),
            ("running tab", .running, "", ["#TODAY", "a"]),
            ("failed tab", .failed, "", ["#YESTERDAY", "c"]),
            ("done tab", .done, "", ["#TODAY", "b", "#OLDER", "d"]),
            ("tab composes with filter", .done, "gpt-5.5", ["#OLDER", "d"]),
            ("no match", .all, "zzz", []),
        ]
        for (name, tab, filter, want) in cases {
            let got = runSections(items, tab: tab, terms: filterTerms(filter), now: now, calendar: calendar)
            XCTAssertEqual(summary(got.sections), want, name)
        }
    }

    func testBuildRowsGroupsOutOfOrderItems() {
        let now = date(2026, 9, 26, 12)
        let i = filterFixture(now)
        let got = runSections([i[3], i[0], i[2], i[1]], tab: .all, terms: [], now: now, calendar: calendar)
        XCTAssertEqual(summary(got.sections), ["#TODAY", "a", "b", "#YESTERDAY", "c", "#OLDER", "d"])
    }

    func testTabCountsFollowTheFilter() {
        let now = date(2026, 9, 26, 12)
        let queued = solo(Session(id: "eeeeeeee-5", status: "queued", queuedAt: now, workDir: "/src/orbit-web"))
        let items = filterFixture(now) + [queued]
        let all = runSections(items, tab: .all, terms: [], now: now, calendar: calendar).counts
        XCTAssertEqual(all, [.all: 5, .running: 2, .failed: 1, .done: 2])
        let orbit = runSections(items, tab: .failed, terms: ["orbit"], now: now, calendar: calendar).counts
        XCTAssertEqual(orbit, [.all: 3, .running: 2, .failed: 1, .done: 0])
    }

    func testQueuedRunSectionsByQueueTime() {
        let now = date(2026, 9, 26, 12)
        let q = solo(Session(id: "q", status: "queued", queuedAt: days(-1, from: now)))
        XCTAssertEqual(runSections([q], tab: .all, terms: [], now: now, calendar: calendar).sections.map(\.section), [.yesterday])
    }
}
