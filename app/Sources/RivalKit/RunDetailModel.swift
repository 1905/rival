import Foundation

/// The detail pane's tabs. Result is the parsed answer, Raw the log tail
/// (the TUI's Output).
public enum DetailTab: String, CaseIterable, Sendable {
    case result = "Result", raw = "Raw", prompt = "Prompt", info = "Info"
}

/// View state of the detail pane, kept free of SwiftUI so it can be tested.
///
/// - Opening another run resets to the first member with follow on (TUI
///   `detailPane.open`). A finished member opens on Result, a live one on Raw.
/// - When the shown member goes from live to finished while the user is on
///   Raw with follow on, the tab moves to Result. Scrolled up or on another
///   tab, it stays.
/// - The member is anchored by session id: a refresh that re-sorts the group
///   keeps the member the user picked. When it vanishes, the first member is
///   shown.
/// - Follow is on until the user scrolls away from the tail. Scrolling back to
///   the tail, the toggle, or switching member turns it on again.
public struct RunDetailModel: Equatable, Sendable {
    public private(set) var runID: String?
    public private(set) var memberID: String?
    public private(set) var follow = true
    public var tab: DetailTab = .raw
    /// The member seen at the last `sync` and whether it was live then.
    private var lastSeen: Seen?

    private struct Seen: Equatable, Sendable {
        let id: String
        let live: Bool
    }

    public init() {}

    /// Call with the selected run on every selection change and refresh.
    public mutating func sync(_ item: RunItem) {
        if item.id != runID {
            runID = item.id
            memberID = item.sessions.first?.id
            follow = true
            let live = item.sessions.first.map { isLive($0.status) } ?? false
            tab = live ? .raw : .result
            lastSeen = item.sessions.first.map { Seen(id: $0.id, live: live) }
            return
        }
        if memberID.map({ m in !item.sessions.contains { $0.id == m } }) ?? true {
            memberID = item.sessions.first?.id
        }
        guard let m = member(in: item) else {
            lastSeen = nil
            return
        }
        let live = isLive(m.status)
        if let last = lastSeen, last.id == m.id, last.live, !live, tab == .raw, follow {
            tab = .result
        }
        lastSeen = Seen(id: m.id, live: live)
    }

    /// The member to show: the anchored one when it is still in `item`, else
    /// the first. Safe to call before `sync`.
    public func member(in item: RunItem) -> Session? {
        if item.id == runID, let m = memberID, let s = item.sessions.first(where: { $0.id == m }) { return s }
        return item.sessions.first
    }

    /// The user picked member `id` of `item`: anchors it, turns follow on and
    /// syncs, so no separate `sync` call is needed.
    public mutating func selectMember(id: String, in item: RunItem) {
        if id != memberID {
            memberID = id
            follow = true
        }
        sync(item)
    }

    /// The user scrolled the Output log. Away from the tail pauses follow; back
    /// at the tail resumes it (TUI: `follow = vp.AtBottom()`).
    public mutating func userScrolled(atBottom: Bool) {
        follow = atBottom
    }

    public mutating func setFollow(_ on: Bool) {
        follow = on
    }
}

/// A member's name in the picker: its model id, or "judge" for the consilium
/// judge (TUI `memberLabel`).
public func memberLabel(_ s: Session) -> String {
    s.mode == "consilium" ? "judge" : modelName(s)
}

/// The breadcrumb id: the group id for a group, else the session id, cut to 8.
public func runShortID(_ item: RunItem) -> String {
    guard let s = item.primary else { return "" }
    return String((s.groupID ?? s.id).prefix(8))
}

/// One row of the Info tab.
public struct InfoRow: Hashable, Sendable {
    public let label: String
    public let value: String
}

/// Every stored field of `s`, in the TUI `infoLines` order. Empty values show
/// as "-". Times are local "yyyy-MM-dd HH:mm:ss". The error is not a row; the
/// view shows it below in the failure colour.
public func infoRows(_ s: Session, now: Date, timeZone: TimeZone = .current) -> [InfoRow] {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.timeZone = timeZone
    f.dateFormat = "yyyy-MM-dd HH:mm:ss"
    func ts(_ d: Date?) -> String {
        guard let d, !d.isGoZero else { return "" }
        return f.string(from: d)
    }
    let rows: [(String, String)] = [
        ("id", s.id),
        ("group id", s.groupID ?? ""),
        ("cli", s.cli),
        ("model", modelName(s)),
        ("effort", s.effort),
        ("mode", s.mode),
        ("status", s.status),
        ("exit", s.exitCode.map(String.init) ?? ""),
        ("started", ts(s.startTime)),
        ("ended", ts(s.endTime)),
        ("duration", sessionElapsed(s, now: now)),
        ("queued at", ts(s.queuedAt)),
        ("workdir", s.workDir),
        ("scope", s.reviewScope ?? ""),
        ("account", s.account ?? ""),
        ("pid", s.pid > 0 ? String(s.pid) : ""),
        ("output", "\(s.outputBytes) bytes, \(s.outputLines) lines"),
        ("log", s.logFile),
    ]
    return rows.map { InfoRow(label: $0.0, value: $0.1.isEmpty ? "-" : $0.1) }
}

/// Session counts per status across all runs (TUI `countStatuses`).
public struct SessionStats: Equatable, Sendable {
    public var running = 0, queued = 0, completed = 0, failed = 0, total = 0
    public init() {}
}

public func sessionStats(_ runs: [RunItem]) -> SessionStats {
    var st = SessionStats()
    for item in runs {
        st.total += item.sessions.count
        for s in item.sessions {
            switch s.status {
            case "running": st.running += 1
            case "queued": st.queued += 1
            case "completed": st.completed += 1
            case "failed": st.failed += 1
            default: break
            }
        }
    }
    return st
}

/// The bubbles `spinner.MiniDot` frames at its 12 fps.
public let spinnerFrames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

public func spinnerFrame(at date: Date) -> String {
    let tick = Int((date.timeIntervalSinceReferenceDate * 12).rounded(.down))
    let n = spinnerFrames.count
    return spinnerFrames[((tick % n) + n) % n]
}

/// The one-line result shown after a stop. `marked` is the dead members the
/// app marked failed itself; the rest of `outcome.dead` is left to the rival
/// process that owns them.
public func stopToast(_ outcome: StopOutcome, marked: Int = 0) -> String {
    if outcome.nothingRunning { return "nothing running" }
    func n(_ c: Int, _ one: String, _ many: String) -> String { c == 1 ? one : "\(c) \(many)" }
    var parts: [String] = []
    let pids = outcome.signalledPIDs
    if !pids.isEmpty {
        parts.append("SIGTERM sent to " + (pids.count == 1 ? "pid \(pids[0])" : "\(pids.count) processes"))
    }
    let left = outcome.dead.count - marked
    if marked > 0 {
        parts.append(n(marked, "process already dead", "processes already dead") + " — marked failed")
    }
    if left > 0 {
        parts.append(n(left, "process already dead", "processes already dead") + " — left to its rival process")
    }
    if !outcome.unverified.isEmpty {
        let c = outcome.unverified.count
        parts.append((c == 1 ? "" : "\(c) ") + "not signalled — cannot verify process identity")
    }
    return parts.joined(separator: " · ")
}
