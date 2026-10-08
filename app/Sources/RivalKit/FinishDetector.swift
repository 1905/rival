import Foundation

/// Spots runs that finished between two store snapshots, for the finish
/// notifications. Feed it every published snapshot in order.
///
/// - The first snapshot only records a baseline: the app launching into a
///   history of finished runs must not post a burst of notifications.
/// - A run fires only on a live (running or queued) → completed/failed
///   transition. A run first seen already finished never fires.
public struct FinishDetector: Sendable {
    private var previous: [RunItem]?

    public init() {}

    /// Returns the runs that finished since the last call, in snapshot order.
    public mutating func update(_ runs: [RunItem]) -> [RunItem] {
        defer { previous = runs }
        guard let previous else { return [] }
        return finishedRuns(previous: previous, current: runs)
    }
}

/// The runs in `current` that were running or queued in `previous` and are now
/// completed or failed. A group counts as finished once its reduced status is
/// (every member done), not when one member ends.
public func finishedRuns(previous: [RunItem], current: [RunItem]) -> [RunItem] {
    var before: [String: RunStatus] = [:]
    for item in previous { before[item.id] = runStatus(item) }
    return current.filter { item in
        guard let was = before[item.id], was.isLive else { return false }
        let now = runStatus(item)
        return now == .completed || now == .failed
    }
}

/// The text of one finish notification.
public struct FinishNote: Equatable, Sendable {
    public let runID: String
    /// "✓ review orbit-web · gpt-6-astra · 6m24s" or
    /// "✗ failed review orbit-web · gpt-6-astra · 6m24s".
    public let title: String
    /// The first line of a failed member's error, else the prompt preview.
    /// Empty when there is neither.
    public let body: String
}

public func finishNote(_ item: RunItem, now: Date = Date()) -> FinishNote {
    let failed = runStatus(item) == .failed
    let parts = [
        runKind(item),
        projectName(item.primary?.workDir ?? ""),
    ].joined(separator: " ")
    let title = (failed ? "✗ failed " : "✓ ") + [parts, runModelName(item), runElapsed(item, now: now)]
        .joined(separator: " · ")

    var body = ""
    if failed, let err = item.sessions.first(where: { $0.status == "failed" && $0.error != nil })?.error {
        body = firstLine(err)
    } else if let preview = item.primary?.promptPreview {
        body = firstLine(preview)
    }
    return FinishNote(runID: item.id, title: title, body: String(body.prefix(200)))
}

private func firstLine(_ s: String) -> String {
    sanitizeLog(s).split(separator: "\n").lazy
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .first { !$0.isEmpty } ?? ""
}

// MARK: - Menu bar

/// When a run ended: the latest member end time. A member without one ends at
/// start + duration; a run with neither falls back to `runTime`.
public func runEndTime(_ item: RunItem) -> Date {
    var latest: Date?
    for s in item.sessions {
        var end = s.endTime
        if end == nil || end!.isZeroTime, !s.startTime.isZeroTime, let d = s.duration, let secs = parseDuration(d) {
            end = s.startTime.addingTimeInterval(secs)
        }
        if let e = end, !e.isZeroTime, latest == nil || e > latest! { latest = e }
    }
    return latest ?? runTime(item)
}

/// What the menu bar popover lists: every live run in store order (newest
/// first), and the `recentLimit` most recently ended finished runs.
public func menuBarRuns(_ runs: [RunItem], recentLimit: Int = 5) -> (live: [RunItem], recent: [RunItem]) {
    var live: [RunItem] = []
    var done: [(item: RunItem, end: Date)] = []
    for item in runs {
        switch runStatus(item) {
        case .running, .queued: live.append(item)
        case .completed, .failed: done.append((item, runEndTime(item)))
        case .unknown: break
        }
    }
    // A stable sort: equal end times keep store order.
    let recent = done.enumerated()
        .sorted { $0.element.end != $1.element.end ? $0.element.end > $1.element.end : $0.offset < $1.offset }
        .prefix(max(0, recentLimit))
        .map(\.element.item)
    return (live, recent)
}
