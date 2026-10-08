import Foundation

/// The status filter above the list (TUI `statusTab`).
public enum StatusTab: CaseIterable, Sendable {
    case all, running, failed, done

    public var label: String {
        switch self {
        case .all: return "ALL"
        case .running: return "RUNNING"
        case .failed: return "FAILED"
        case .done: return "DONE"
        }
    }

    /// Queued runs sit under RUNNING: both are "not finished yet".
    public func accepts(_ status: RunStatus) -> Bool {
        switch self {
        case .all: return true
        case .running: return status.isLive
        case .failed: return status == .failed
        case .done: return status == .completed
        }
    }
}

public enum DaySection: String, CaseIterable, Sendable {
    case today = "TODAY", yesterday = "YESTERDAY", thisWeek = "THIS WEEK", older = "OLDER"
}

/// Buckets `date` relative to `now` by local calendar day (TUI `sectionFor`).
/// THIS WEEK is the five days before yesterday. Boundaries are calendar day
/// starts, so a DST switch cannot shift them by an hour. The zero time
/// 0001-01-01T00:00:00Z that older CLI versions wrote is OLDER.
public func section(for date: Date, now: Date, calendar: Calendar) -> DaySection {
    if date.isZeroTime { return .older }
    let today = calendar.startOfDay(for: now)
    func dayStart(_ offset: Int) -> Date {
        calendar.date(byAdding: .day, value: offset, to: today).map(calendar.startOfDay) ?? today
    }
    if date >= today { return .today }
    if date >= dayStart(-1) { return .yesterday }
    if date >= dayStart(-6) { return .thisWeek }
    return .older
}

/// Splits a filter into lowercase, AND-ed terms.
public func filterTerms(_ filter: String) -> [String] {
    filter.lowercased().split(whereSeparator: { $0.isWhitespace }).map(String.init)
}

/// True when every term occurs, case-insensitively, in the run's searchable
/// text: status, kind, and each member's status, model, effort, project,
/// prompt preview, review scope and short id (TUI `matchesFilter`).
public func matches(_ item: RunItem, terms: [String]) -> Bool {
    terms.allSatisfy { item.haystack.contains($0.lowercased()) }
}

/// The run's searchable text, lowercase. Fields are joined with "\n", which
/// no term can contain, so a match never spans two fields. `RunItem.init`
/// stores it.
func filterHaystack(_ sessions: [Session], status: String, kind: String) -> String {
    var parts = [status, kind]
    for s in sessions {
        parts += [s.status, modelName(s), s.effort, projectName(s.workDir),
                  s.promptPreview ?? "", s.reviewScope ?? "", String(s.id.prefix(8))]
    }
    return parts.joined(separator: "\n").lowercased()
}

/// One day section of the filtered list.
public struct RunSection: Hashable, Sendable {
    public let section: DaySection
    public let items: [RunItem]
}

/// Applies the status tab and the filter, then groups what is left under day
/// sections (TUI `rowsAndCounts`). Empty sections are omitted and items keep
/// their relative order. `counts` are the filter matches per tab.
public func runSections(
    _ items: [RunItem], tab: StatusTab, terms: [String], now: Date, calendar: Calendar
) -> (sections: [RunSection], counts: [StatusTab: Int]) {
    var counts = Dictionary(uniqueKeysWithValues: StatusTab.allCases.map { ($0, 0) })
    var buckets: [DaySection: [RunItem]] = [:]
    for item in items where matches(item, terms: terms) {
        let status = runStatus(item)
        for t in StatusTab.allCases where t.accepts(status) { counts[t, default: 0] += 1 }
        guard tab.accepts(status) else { continue }
        buckets[section(for: runTime(item), now: now, calendar: calendar), default: []].append(item)
    }
    let sections = DaySection.allCases.compactMap { sec in
        buckets[sec].map { RunSection(section: sec, items: $0) }
    }
    return (sections, counts)
}
