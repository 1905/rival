import Foundation

// Numbered pages of runs, shared with the TUI list (plan P5b):
// - 50 runs per page; section headers do not count.
// - A page shows the section headers of the runs on it.
// - Pages are 0-based here; the footer shows them 1-based.

public let pageSize = 50

/// Pages needed for `total` runs. Never less than 1, so an empty list still
/// has a page 0 to sit on.
public func pageCount(total: Int, size: Int = pageSize) -> Int {
    max(1, (max(0, total) + size - 1) / size)
}

/// The runs on `page`, clamped to the valid pages.
public func pageSlice<T>(runs: [T], page: Int, size: Int = pageSize) -> [T] {
    let p = min(max(page, 0), pageCount(total: runs.count, size: size) - 1)
    let start = p * size
    guard start < runs.count else { return [] }
    return Array(runs[start..<min(start + size, runs.count)])
}

/// The day sections of one page: the flat run order of `sections` sliced to
/// `page`, regrouped under the sections those runs came from. A section split
/// across two pages shows its header on both.
public func pageSections(_ sections: [RunSection], page: Int, size: Int = pageSize) -> [RunSection] {
    let flat = sections.flatMap { sec in sec.items.map { (sec.section, $0) } }
    var out: [RunSection] = []
    for (sec, item) in pageSlice(runs: flat, page: page, size: size) {
        if let last = out.last, last.section == sec {
            out[out.count - 1] = RunSection(section: sec, items: last.items + [item])
        } else {
            out.append(RunSection(section: sec, items: [item]))
        }
    }
    return out
}

/// The page that holds `runID`, or nil when the list does not show it.
public func pageFor(runID: String?, in ids: [String], size: Int = pageSize) -> Int? {
    guard let runID, let i = ids.firstIndex(of: runID) else { return nil }
    return i / size
}

/// ↑/↓ in the paged list. `ids` is the whole filtered list, not one page, so
/// a move past the last or first row of a page lands on the next or previous
/// page. The ends clamp. With no selection (or one the filter hides), down
/// picks the first row of `page` and up its last row.
public func movePaged(
    selection: String?, by delta: Int, in ids: [String], page: Int, size: Int = pageSize
) -> (selection: String?, page: Int) {
    guard !ids.isEmpty else { return (selection, 0) }
    if let i = selection.flatMap({ ids.firstIndex(of: $0) }) {
        let j = min(max(i + delta, 0), ids.count - 1)
        return (ids[j], j / size)
    }
    let p = min(max(page, 0), pageCount(total: ids.count, size: size) - 1)
    let onPage = pageSlice(runs: ids, page: p, size: size)
    return (delta >= 0 ? onPage.first : onPage.last, p)
}

/// The current page of the run list. The list holds one and feeds it every
/// data, selection and filter change.
public struct PageState: Equatable, Sendable {
    public private(set) var page: Int
    public let size: Int

    public init(page: Int = 0, size: Int = pageSize) {
        self.page = max(0, page)
        self.size = size
    }

    public func count(total: Int) -> Int { pageCount(total: total, size: size) }

    /// Keeps the page inside the list after it shrank.
    public mutating func clamp(total: Int) {
        page = min(max(page, 0), count(total: total) - 1)
    }

    /// A tab or filter change: back to page 1.
    public mutating func reset() { page = 0 }

    /// After a selection change: show the page that holds the selected run.
    /// A hidden run (or none) leaves the page where it was, clamped.
    public mutating func anchor(selection: String?, in ids: [String]) {
        if let p = pageFor(runID: selection, in: ids, size: size) {
            page = p
        } else {
            clamp(total: ids.count)
        }
    }

    /// After a refresh from `old` to `new` (TUI cursor rules): a run that is
    /// still listed keeps the selection and pulls the page to it. A selected
    /// run that vanished hands the selection to the run now at its old
    /// index, clamped to the new list, and the page follows; that is the
    /// last page only when the old page no longer exists. Returns the
    /// replacement selection, or nil when the selection stays as it is.
    public mutating func refresh(selection: String?, from old: [String], to new: [String]) -> String? {
        if pageFor(runID: selection, in: new, size: size) != nil {
            anchor(selection: selection, in: new)
            return nil
        }
        guard !new.isEmpty, let sel = selection, let i = old.firstIndex(of: sel) else {
            clamp(total: new.count)
            return nil
        }
        let j = min(i, new.count - 1)
        page = j / size
        return new[j]
    }

    /// ←/→ and the footer arrows. Returns the first run of the new page, or
    /// nil when there is no page that way.
    public mutating func turn(by delta: Int, in ids: [String]) -> String? {
        let target = page + delta
        guard delta != 0, target >= 0, target < count(total: ids.count) else { return nil }
        page = target
        return pageSlice(runs: ids, page: page, size: size).first
    }

    /// ↑/↓: the new selection, with the page following it.
    public mutating func move(selection: String?, by delta: Int, in ids: [String]) -> String? {
        let r = movePaged(selection: selection, by: delta, in: ids, page: page, size: size)
        page = r.page
        return r.selection
    }

    /// Home: the first run of page 1.
    public mutating func first(in ids: [String]) -> String? {
        page = 0
        return ids.first
    }

    /// End: the last run of the last page.
    public mutating func last(in ids: [String]) -> String? {
        page = count(total: ids.count) - 1
        return ids.last
    }

    /// `‹ prev  page P/N  next ›  · T runs`, or `T runs` on a single page.
    public func footer(total: Int) -> PageFooter {
        let n = count(total: total)
        let p = min(page, n - 1)
        return PageFooter(page: p + 1, pages: n, total: total, hasPrev: p > 0, hasNext: p < n - 1)
    }
}

/// What the footer under the run list shows.
public struct PageFooter: Equatable, Sendable {
    public let page: Int
    public let pages: Int
    public let total: Int
    public let hasPrev: Bool
    public let hasNext: Bool

    public var runsLabel: String { total == 1 ? "1 run" : "\(total) runs" }
    public var pageLabel: String { "page \(page)/\(pages)" }
    public var paged: Bool { pages > 1 }

    /// The footer as one line, for tests and accessibility.
    public var text: String {
        paged ? "‹ prev  \(pageLabel)  next ›  · \(runsLabel)" : runsLabel
    }
}

// MARK: - Menu bar

/// LIVE rows in the menu bar popover before "+N more — open Rival".
public let menuBarLiveCap = 10

/// The first `cap` live runs and how many more were cut.
public func capLive(_ live: [RunItem], cap: Int = menuBarLiveCap) -> (shown: [RunItem], more: Int) {
    let n = max(0, cap)
    return (Array(live.prefix(n)), max(0, live.count - n))
}
