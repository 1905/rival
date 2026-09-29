import XCTest
@testable import RivalKit

/// The same cases as the TUI list pagination (plan P5b): slicing with
/// sections, moves across pages, bounds, reset, anchoring, the footer and the
/// menu bar cap. Most cases use pages of 3 so the lists stay readable.
final class PaginationTests: XCTestCase {
    let ids = ["a", "b", "c", "d", "e", "f", "g"]  // pages of 3: [a b c] [d e f] [g]

    func item(_ id: String, _ status: String = "completed") -> RunItem { solo(Session(id: id, status: status)) }

    func testPageCount() {
        XCTAssertEqual(pageCount(total: 0), 1)
        XCTAssertEqual(pageCount(total: 1), 1)
        XCTAssertEqual(pageCount(total: 50), 1)
        XCTAssertEqual(pageCount(total: 51), 2)
        XCTAssertEqual(pageCount(total: 120), 3)
        XCTAssertEqual(pageSize, 50)
    }

    func testPageSliceBounds() {
        XCTAssertEqual(pageSlice(runs: ids, page: 0, size: 3), ["a", "b", "c"])
        XCTAssertEqual(pageSlice(runs: ids, page: 2, size: 3), ["g"])
        XCTAssertEqual(pageSlice(runs: ids, page: -1, size: 3), ["a", "b", "c"], "below page 1 clamps")
        XCTAssertEqual(pageSlice(runs: ids, page: 9, size: 3), ["g"], "past the end clamps to the last page")
        XCTAssertEqual(pageSlice(runs: [String](), page: 0), [])
    }

    func testPageSectionsSplitsASectionAcrossPages() {
        let sections = [
            RunSection(section: .today, items: [item("a"), item("b")]),
            RunSection(section: .yesterday, items: [item("c"), item("d"), item("e")]),
            RunSection(section: .older, items: [item("f")]),
        ]
        let p0 = pageSections(sections, page: 0, size: 3)
        XCTAssertEqual(p0.map(\.section), [.today, .yesterday])
        XCTAssertEqual(p0.map { $0.items.map(\.id) }, [["solo:a", "solo:b"], ["solo:c"]])
        let p1 = pageSections(sections, page: 1, size: 3)
        XCTAssertEqual(p1.map(\.section), [.yesterday, .older], "a split section repeats its header")
        XCTAssertEqual(p1.map { $0.items.map(\.id) }, [["solo:d", "solo:e"], ["solo:f"]])
    }

    /// 120 runs through the real section logic: 50 / 50 / 20, and a page's
    /// headers are only those of its runs.
    func testPageSectionsWithRunSections() {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "Europe/Berlin")!
        let now = cal.date(from: DateComponents(year: 2026, month: 9, day: 26, hour: 12))!
        // Newest first, one run an hour: 12 today, 24 yesterday, the rest this week.
        let runs = (0..<120).map { i in
            solo(Session(id: String(format: "r%03d", i), status: "completed",
                         startTime: now.addingTimeInterval(-3600 * Double(i) - 60)))
        }
        let (sections, _) = runSections(runs, tab: .all, terms: [], now: now, calendar: cal)
        let flat = sections.flatMap { $0.items.map(\.id) }
        XCTAssertEqual(flat.count, 120)
        XCTAssertEqual(pageSections(sections, page: 0).flatMap { $0.items.map(\.id) }, Array(flat[0..<50]))
        XCTAssertEqual(pageSections(sections, page: 0).map(\.section), [.today, .yesterday, .thisWeek])
        let last = pageSections(sections, page: 2)
        XCTAssertEqual(last.flatMap(\.items).count, 20)
        XCTAssertEqual(last.map(\.section), [.thisWeek])
    }

    func testMovePagedCrossesPageEdges() {
        XCTAssertEqual(movePaged(selection: "c", by: 1, in: ids, page: 0, size: 3).selection, "d")
        XCTAssertEqual(movePaged(selection: "c", by: 1, in: ids, page: 0, size: 3).page, 1)
        XCTAssertEqual(movePaged(selection: "d", by: -1, in: ids, page: 1, size: 3).selection, "c")
        XCTAssertEqual(movePaged(selection: "d", by: -1, in: ids, page: 1, size: 3).page, 0)
        XCTAssertEqual(movePaged(selection: "a", by: 1, in: ids, page: 0, size: 3).page, 0)
    }

    func testMovePagedClampsAtTheEnds() {
        let top = movePaged(selection: "a", by: -1, in: ids, page: 0, size: 3)
        XCTAssertEqual(top.selection, "a")
        XCTAssertEqual(top.page, 0)
        let bottom = movePaged(selection: "g", by: 1, in: ids, page: 2, size: 3)
        XCTAssertEqual(bottom.selection, "g")
        XCTAssertEqual(bottom.page, 2)
    }

    func testMovePagedWithoutSelectionStaysOnThePage() {
        XCTAssertEqual(movePaged(selection: nil, by: 1, in: ids, page: 1, size: 3).selection, "d")
        XCTAssertEqual(movePaged(selection: "hidden", by: -1, in: ids, page: 1, size: 3).selection, "f")
        XCTAssertEqual(movePaged(selection: nil, by: 1, in: ids, page: 7, size: 3).page, 2)
        XCTAssertEqual(movePaged(selection: "x", by: 1, in: [], page: 3).selection, "x")
        XCTAssertEqual(movePaged(selection: "x", by: 1, in: [], page: 3).page, 0)
    }

    func testPageStateMoveFollowsTheSelection() {
        var st = PageState(size: 3)
        XCTAssertEqual(st.move(selection: "c", by: 1, in: ids), "d")
        XCTAssertEqual(st.page, 1)
        XCTAssertEqual(st.move(selection: "d", by: -1, in: ids), "c")
        XCTAssertEqual(st.page, 0)
    }

    func testTurnBounds() {
        var st = PageState(size: 3)
        XCTAssertNil(st.turn(by: -1, in: ids), "no page before page 1")
        XCTAssertEqual(st.page, 0)
        XCTAssertEqual(st.turn(by: 1, in: ids), "d", "a page turn selects the page's first run")
        XCTAssertEqual(st.turn(by: 1, in: ids), "g")
        XCTAssertEqual(st.page, 2)
        XCTAssertNil(st.turn(by: 1, in: ids), "no page after the last")
        XCTAssertEqual(st.page, 2)
        XCTAssertEqual(st.turn(by: -1, in: ids), "d")
        var single = PageState()
        XCTAssertNil(single.turn(by: 1, in: ids), "one page: nothing to turn")
    }

    func testHomeAndEnd() {
        var st = PageState(page: 1, size: 3)
        XCTAssertEqual(st.last(in: ids), "g")
        XCTAssertEqual(st.page, 2)
        XCTAssertEqual(st.first(in: ids), "a")
        XCTAssertEqual(st.page, 0)
    }

    func testReset() {
        var st = PageState(page: 2, size: 3)
        st.reset()
        XCTAssertEqual(st.page, 0)
    }

    /// Two new runs arrive above the selected one and push it to page 2.
    func testAnchorAfterInsertAbove() {
        var st = PageState(size: 3)
        st.anchor(selection: "c", in: ids)
        XCTAssertEqual(st.page, 0)
        st.anchor(selection: "c", in: ["new1", "new2"] + ids)
        XCTAssertEqual(st.page, 1)
    }

    func testAnchorHiddenRunKeepsThePage() {
        var st = PageState(size: 3)
        st.anchor(selection: "g", in: ids)
        XCTAssertEqual(st.page, 2)
        st.anchor(selection: "hidden", in: ids)
        XCTAssertEqual(st.page, 2, "a selection outside the list leaves the page")
        st.anchor(selection: nil, in: ["a", "b", "c", "d", "e"])
        XCTAssertEqual(st.page, 1, "clamped when the list shrank")
        st.anchor(selection: nil, in: [])
        XCTAssertEqual(st.page, 0)
    }

    func testRefreshKeepsAListedSelection() {
        var st = PageState(size: 3)
        XCTAssertNil(st.refresh(selection: "c", from: ids, to: ["n1", "n2"] + ids))
        XCTAssertEqual(st.page, 1, "an insert above pushed the run to page 2")
    }

    /// The TUI cursor rule: the run at the vanished run's old index takes the
    /// selection, and the page follows it.
    func testRefreshVanishedRunClampsTheIndex() {
        var st = PageState(page: 1, size: 3)
        XCTAssertEqual(st.refresh(selection: "e", from: ids, to: ["a", "b", "c", "d", "f", "g"]), "f")
        XCTAssertEqual(st.page, 1, "the same page while it still exists")

        st = PageState(page: 2, size: 3)
        XCTAssertEqual(st.refresh(selection: "g", from: ids, to: ["a", "b", "c", "d", "e"]), "e")
        XCTAssertEqual(st.page, 1, "the old page is gone: the last page")

        st = PageState(page: 2, size: 3)
        XCTAssertNil(st.refresh(selection: "g", from: ids, to: []))
        XCTAssertEqual(st.page, 0)

        st = PageState(page: 2, size: 3)
        XCTAssertNil(st.refresh(selection: nil, from: ids, to: ["a", "b", "c", "d"]))
        XCTAssertEqual(st.page, 1, "no selection: clamp only")
    }

    func testFooter() {
        XCTAssertEqual(PageState().footer(total: 0).text, "0 runs")
        XCTAssertEqual(PageState().footer(total: 1).text, "1 run")
        XCTAssertEqual(PageState().footer(total: 50).text, "50 runs")
        let mid = PageState(page: 1).footer(total: 120)
        XCTAssertEqual(mid.text, "‹ prev  page 2/3  next ›  · 120 runs")
        XCTAssertTrue(mid.hasPrev)
        XCTAssertTrue(mid.hasNext)
        let first = PageState().footer(total: 120)
        XCTAssertFalse(first.hasPrev)
        XCTAssertTrue(first.hasNext)
        let last = PageState(page: 9).footer(total: 120)
        XCTAssertEqual(last.page, 3, "a stale page shows as the last one")
        XCTAssertFalse(last.hasNext)
    }

    func testMenuBarLiveCap() {
        let live = (0..<13).map { item("l\($0)", "running") }
        let runs = live + (0..<8).map { item("d\($0)") }
        let (all, recent) = menuBarRuns(runs)
        XCTAssertEqual(all.count, 13)
        XCTAssertEqual(recent.count, 5, "RECENT stays at 5")
        let (shown, more) = capLive(all)
        XCTAssertEqual(shown.map(\.id), live.prefix(10).map(\.id))
        XCTAssertEqual(more, 3)
        XCTAssertEqual(capLive(Array(all.prefix(4))).more, 0)
        XCTAssertEqual(capLive(Array(all.prefix(10))).more, 0)
        XCTAssertEqual(menuBarLiveCap, 10)
    }
}
