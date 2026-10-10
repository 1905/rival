import Foundation

/// One row of the run list: a multi-model group or a single standalone run.
/// Ports `sessionview.Bucket` plus the TUI's `displayItem`.
public struct RunItem: Identifiable, Hashable, Sendable {
    /// "group:<GroupID>" for a group, "solo:<SessionID>" for a standalone run
    /// (the TUI's `itemKey`). Stable across refreshes; use it for selection.
    public let id: String
    /// Members in `sortGroupMembers` order: reviewers first, the judge last.
    public let sessions: [Session]
    /// The row status (`runStatus`), the KIND cell (`runKind`) and the
    /// lowercase filter haystack (`matches`), computed once here: the list
    /// filters and draws every run on each refresh.
    public let status: RunStatus
    public let kind: String
    let haystack: String

    public init(id: String, sessions: [Session]) {
        self.id = id
        self.sessions = sessions
        let isGroup = sessions.count > 1 || (sessions.count == 1 && sessions[0].groupID != nil)
        let statusString = isGroup ? groupStatus(sessions) : sessions.first?.status ?? ""
        status = RunStatus(rawValue: statusString) ?? .unknown
        kind = kindCell(sessions, isGroup: isGroup)
        haystack = filterHaystack(sessions, status: statusString, kind: kind)
    }

    /// The first member, which carries the shared metadata.
    public var primary: Session? { sessions.first }

    /// True for a logical group, including a degraded one where only one of the
    /// requested models passed preflight (TUI `displayItem.IsGroup`).
    public var isGroup: Bool {
        sessions.count > 1 || (sessions.count == 1 && sessions[0].groupID != nil)
    }
}

public enum RunStatus: String, Sendable {
    case running, queued, completed, failed, unknown

    /// "Not finished yet": running, or queued and waiting for a slot.
    public var isLive: Bool { self == .running || self == .queued }
}

/// "Not finished yet": running, or queued and waiting for a slot.
public func isLive(_ status: String) -> Bool {
    status == "running" || status == "queued"
}

// MARK: - Grouping (sessionview.Group)

/// Buckets sessions by group id, keeping the order in which each key first
/// appears. A session without a group id is its own bucket. Members are sorted
/// with `sortGroupMembers`.
public func groupRuns(_ sessions: [Session]) -> [RunItem] {
    var buckets: [String: [Session]] = [:]
    var order: [String] = []
    for s in sessions {
        let key = s.groupID.map { "group:" + $0 } ?? "solo:" + s.id
        if buckets[key] == nil { order.append(key) }
        buckets[key, default: []].append(s)
    }
    return order.map { RunItem(id: $0, sessions: sortGroupMembers(buckets[$0] ?? [])) }
}

/// Restores the order in which a grouped run requested its models
/// (`session.SortGroupMembers`): the consilium judge last, then queue time,
/// then model rank, start time and id.
public func sortGroupMembers(_ sessions: [Session]) -> [Session] {
    sessions.sorted { a, b in
        let ma = a.mode == "consilium" ? 1 : 0, mb = b.mode == "consilium" ? 1 : 0
        if ma != mb { return ma < mb }
        if let qa = a.queuedAt, let qb = b.queuedAt, qa != qb { return qa < qb }
        let ra = groupModelRank(a), rb = groupModelRank(b)
        if ra != rb { return ra < rb }
        if a.startTime != b.startTime { return a.startTime < b.startTime }
        return a.id < b.id
    }
}

private func groupModelRank(_ s: Session) -> Int {
    switch engineLabel(cli: s.cli, model: s.model) {
    case "sol": return 0
    case "kimi-k3": return 1
    case "claude": return 2
    case "grok": return 3
    default: return 100
    }
}

/// Port of `config.EngineLabel`: the public reviewer label for a session. Used
/// only for member ordering; the UI shows the raw model id.
public func engineLabel(cli: String, model: String) -> String {
    switch model {
    case "gpt-6.1-sol", "gpt-5.6-sol": return "sol"
    case "gpt-6-astra": return "codex"
    case "claude-opus-5-5": return "claude"
    case "claude-fable-5-1": return "fable"
    case "moonshotai/kimi-k3": return "kimi-k3"
    case "grok-4.6": return "grok"
    case "x-ai/grok-4.6": return "grok-4.6-openrouter"
    default: break
    }
    switch cli {
    case "codex": return "sol"
    case "grok": return "grok"
    case "claude", "fable", "opencode": return "retired-model"
    default: break
    }
    if !model.isEmpty { return modelLabel(model) }
    return cli
}

private func modelLabel(_ model: String) -> String {
    switch model {
    case "gpt-6.1-sol", "gpt-5.6-sol", "sol": return "sol"
    case "gpt-6-astra", "codex": return "codex"
    case "claude-opus-5-5", "claude": return "claude"
    case "claude-fable-5-1", "fable": return "fable"
    case "moonshotai/kimi-k3", "kimi-k3": return "kimi-k3"
    case "grok-4.6", "grok": return "grok"
    case "x-ai/grok-4.6", "grok-4.6-openrouter": return "grok-4.6-openrouter"
    default: return "retired-model"
    }
}

// MARK: - Status

/// The row status. A group reduces its members with the `sessionview.Status`
/// tier (running > queued > failed > completed). A solo run shows its own
/// status, and one the app does not know maps to `.unknown` (TUI `itemStatus`).
public func runStatus(_ item: RunItem) -> RunStatus { item.status }

/// `sessionview.Status`.
func groupStatus(_ sessions: [Session]) -> String {
    for tier in ["running", "queued", "failed"] where sessions.contains(where: { $0.status == tier }) {
        return tier
    }
    return "completed"
}

/// The TUI `statusGlyph`: each status has its own shape, so it reads without
/// colour. `spin` is the running spinner frame.
public func statusGlyph(_ status: RunStatus, spin: String = "⠋") -> String {
    switch status {
    case .running: return spin
    case .queued: return "◌"
    case .completed: return "✓"
    case .failed: return "✗"
    case .unknown: return "·"
    }
}

// MARK: - Kind (TUI kindLabel)

/// The KIND cell: review | plan | mega | sec | slop | raw, plus "/dk" for a
/// docker Claude run. A group classifies through `sessionview.Kind`; a solo run
/// maps its own mode. An unrecognised mode is shown verbatim.
public func runKind(_ item: RunItem) -> String { item.kind }

private func kindCell(_ sessions: [Session], isGroup: Bool) -> String {
    if isGroup { return shortKind(groupKind(sessions)) }
    guard let s = sessions.first else { return "" }
    var kind = shortKind(s.mode)
    if s.mode == "docker" && (s.cli == "claude" || s.cli == "fable") {
        kind += "/dk"
    }
    return kind
}

/// `sessionview.Kind`: security, then plan, else megareview.
func groupKind(_ sessions: [Session]) -> String {
    for mode in ["security", "plan"] where sessions.contains(where: { $0.mode == mode }) {
        return mode
    }
    return "megareview"
}

func shortKind(_ mode: String) -> String {
    switch mode {
    case "megareview", "consilium": return "mega"
    case "security": return "sec"
    case "plan": return "plan"
    case "raw": return "raw"
    case "", "review", "native", "docker": return "review"
    default: return mode
    }
}

// MARK: - Cells

/// The stored model id verbatim, or the CLI when no model was recorded.
public func modelName(_ s: Session) -> String {
    s.model.isEmpty ? s.cli : s.model
}

/// The MODEL cell: the first member's model plus "+N" for other distinct ids.
public func runModelName(_ item: RunItem) -> String {
    guard let first = item.primary else { return "" }
    let name = modelName(first)
    let distinct = Set(item.sessions.map(modelName)).subtracting([name]).count
    return distinct > 0 ? "\(name) +\(distinct)" : name
}

/// `sessionview.Effort`: the shared effort, or "mixed" when members differ.
public func runEffort(_ item: RunItem) -> String {
    guard let first = item.sessions.first else { return "" }
    return item.sessions.dropFirst().allSatisfy({ $0.effort == first.effort }) ? first.effort : "mixed"
}

/// The last element of the work dir (trailing slashes ignored), or "-" when empty.
public func projectName(_ workDir: String) -> String {
    if workDir.isEmpty { return "-" }
    var trimmed = Substring(workDir)
    while trimmed.count > 1, trimmed.hasSuffix("/") { trimmed = trimmed.dropLast() }
    if trimmed == "/" { return "/" }
    if let slash = trimmed.lastIndex(of: "/") { return String(trimmed[trimmed.index(after: slash)...]) }
    return String(trimmed)
}

/// When a run appeared: its start, or its queue time when it has not started.
public func runTime(_ item: RunItem) -> Date {
    guard let s = item.primary else { return .zeroTime }
    if s.startTime.isZeroTime, let q = s.queuedAt { return q }
    return s.startTime
}

// MARK: - Elapsed (sessionview.Elapsed)

/// The wall-clock span of the run: earliest member start to latest member end,
/// rounded to the second and formatted as the CLI prints durations ("1m23s"). A
/// running or queued member extends to `now`; a queued member counts from its
/// queue time. "-" when nothing has started.
public func runElapsed(_ item: RunItem, now: Date) -> String {
    elapsed(item.sessions, now: now)
}

/// `runElapsed` for one session on its own.
public func sessionElapsed(_ s: Session, now: Date) -> String {
    elapsed([s], now: now)
}

private func elapsed(_ sessions: [Session], now: Date) -> String {
    var earliest: Date?
    var latest: Date?
    for s in sessions {
        var start = s.startTime
        if let q = s.queuedAt, start.isZeroTime || q < start { start = q }
        if start.isZeroTime { continue }

        var end = start
        if isLive(s.status) {
            end = now
        } else if let e = s.endTime {
            end = e
        } else if let d = s.duration, let secs = parseDuration(d) {
            end = start.addingTimeInterval(secs)
        }
        if end < start { end = start }
        if earliest == nil || start < earliest! { earliest = start }
        if latest == nil || end > latest! { latest = end }
    }
    guard let e = earliest, let l = latest, l > e else { return "-" }
    let seconds = Int(l.timeIntervalSince(e).rounded(.toNearestOrAwayFromZero))
    return formatDuration(seconds: seconds)
}

/// The TIME cell: `runElapsed`, prefixed with "#N " for a queued solo run.
public func runTimeLabel(_ item: RunItem, now: Date) -> String {
    let elapsed = runElapsed(item, now: now)
    if !item.isGroup, let s = item.primary, s.status == "queued", let pos = s.queuePosition, pos > 0 {
        return "#\(pos) \(elapsed)"
    }
    return elapsed
}

/// A whole number of seconds as the CLI prints durations: "0s", "45s",
/// "7m0s", "1h2m3s".
public func formatDuration(seconds: Int) -> String {
    if seconds == 0 { return "0s" }
    let sign = seconds < 0 ? "-" : ""
    let n = abs(seconds)
    let h = n / 3600, m = (n % 3600) / 60, s = n % 60
    if h > 0 { return "\(sign)\(h)h\(m)m\(s)s" }
    if m > 0 { return "\(sign)\(m)m\(s)s" }
    return "\(sign)\(s)s"
}

/// Parses the CLI's duration syntax ("1m23s", "1.5h", "300ms") into
/// seconds. Returns nil on malformed input.
public func parseDuration(_ text: String) -> TimeInterval? {
    var s = Substring(text)
    var sign = 1.0
    if let f = s.first, f == "-" || f == "+" {
        sign = f == "-" ? -1 : 1
        s = s.dropFirst()
    }
    if s == "0" { return 0 }
    if s.isEmpty { return nil }
    let units: [(String, Double)] = [
        ("ns", 1e-9), ("us", 1e-6), ("µs", 1e-6), ("μs", 1e-6), ("ms", 1e-3),
        ("h", 3600), ("m", 60), ("s", 1),
    ]
    var total = 0.0
    while !s.isEmpty {
        let numEnd = s.firstIndex(where: { !($0.isASCII && ($0.isNumber || $0 == ".")) }) ?? s.endIndex
        guard numEnd > s.startIndex, let value = Double(s[s.startIndex..<numEnd]) else { return nil }
        s = s[numEnd...]
        guard let unit = units.first(where: { s.hasPrefix($0.0) }) else { return nil }
        total += value * unit.1
        s = s.dropFirst(unit.0.count)
    }
    return sign * total
}
