import Darwin
import Foundation

/// Reads a process's start time, the guard against PID reuse.
public protocol ProcessInspector {
    /// Start time of `pid` in Unix nanoseconds (the unit of Go's
    /// `procinfo.StartNanos` and the session's `pid_start`), or nil when no
    /// such process exists.
    func startTime(pid: Int32) -> Int64?
}

/// Delivers the stop signal.
public protocol Signaller {
    /// Sends SIGTERM. Returns false when the signal could not be delivered.
    func terminate(pid: Int32) -> Bool
}

/// `sysctl(KERN_PROC_PID)` → `kp_proc.p_starttime`, converted exactly as
/// `rival/internal/procinfo/start_darwin.go` does: `sec*1e9 + usec*1000`.
public struct SystemProcessInspector: ProcessInspector {
    public init() {}

    public func startTime(pid: Int32) -> Int64? {
        guard pid > 0 else { return nil }
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
        // A missing PID succeeds with size 0; Go's SysctlKinfoProc rejects any
        // size other than a full kinfo_proc, and so does this.
        guard sysctl(&mib, u_int(mib.count), &info, &size, nil, 0) == 0,
              size == MemoryLayout<kinfo_proc>.stride
        else { return nil }
        // `p_starttime` is a C macro for `p_un.__p_starttime`.
        let tv = info.kp_proc.p_un.__p_starttime
        return Int64(tv.tv_sec) * 1_000_000_000 + Int64(tv.tv_usec) * 1000
    }
}

/// `kill(pid, SIGTERM)`.
public struct SystemSignaller: Signaller {
    public init() {}

    public func terminate(pid: Int32) -> Bool {
        pid > 0 && kill(pid, SIGTERM) == 0
    }
}

/// What a stop would do to one live member.
public enum StopTargetState: Equatable, Sendable {
    /// The PID still has the recorded non-zero `pid_start`: SIGTERM is allowed.
    case signal
    /// No such process, or its PID now belongs to another process.
    case gone
    /// The process exists but the session has no recorded `pid_start`, so the
    /// app cannot tell it from a process that reused the PID. No signal.
    case unverified
}

/// Stop authorization for one session. A missing `pid_start` never
/// authorizes a signal.
public func stopTargetState(_ session: Session, inspector: ProcessInspector) -> StopTargetState {
    guard session.pid > 0, let started = inspector.startTime(pid: session.pid) else { return .gone }
    guard let want = session.pidStart, want != 0 else { return .unverified }
    return started == want ? .signal : .gone
}

public struct StopOutcome: Equatable, Sendable {
    /// Session ids that got SIGTERM, and their PIDs (same order).
    public var signalled: [String] = []
    public var signalledPIDs: [Int32] = []
    /// Session ids whose process is gone, belongs to someone else now, or
    /// could not be signalled.
    public var dead: [String] = []
    /// Session ids with a live PID but no recorded start time. Not signalled.
    public var unverified: [String] = []

    public init(signalled: [String] = [], signalledPIDs: [Int32] = [], dead: [String] = [], unverified: [String] = []) {
        self.signalled = signalled
        self.signalledPIDs = signalledPIDs
        self.dead = dead
        self.unverified = unverified
    }

    /// No member was running or queued with a known PID.
    public var nothingRunning: Bool { signalled.isEmpty && dead.isEmpty && unverified.isEmpty }
}

/// The members a stop concerns: running or queued, with a recorded PID. The
/// one rule behind the Stop button, the confirm sheet and the confirmed stop.
public func stopCandidates(_ item: RunItem) -> [Session] {
    item.sessions.filter { isLive($0.status) && $0.pid > 0 }
}

/// Stops a run: SIGTERM to every live member whose PID still has its recorded
/// non-zero start time. A member whose process died, or whose PID now belongs
/// to another process, goes to `dead`. A member with no recorded start time and
/// a live PID goes to `unverified` and gets no signal.
///
/// This never writes session files. The rival process that owns a signalled
/// run finalizes it itself; see `deadToMark` for the members the app may mark.
public func stop(_ item: RunItem, inspector: ProcessInspector, signaller: Signaller) -> StopOutcome {
    var out = StopOutcome()
    for s in stopCandidates(item) {
        switch stopTargetState(s, inspector: inspector) {
        case .signal where signaller.terminate(pid: s.pid):
            out.signalled.append(s.id)
            out.signalledPIDs.append(s.pid)
        case .unverified:
            out.unverified.append(s.id)
        default:
            out.dead.append(s.id)
        }
    }
    return out
}

/// The reaper rule (`rival/internal/session/reaper.go`): true when the rival
/// process that owns `session` is gone, so nobody else will finalize it. A
/// session without a recorded owner (older releases) counts as unowned. An
/// owner PID that exists without a recorded start counts as alive, like Go's
/// `procinfo.Alive(pid, 0)`.
public func ownerGone(_ session: Session, inspector: ProcessInspector) -> Bool {
    guard let owner = session.ownerPID, owner > 0 else { return true }
    guard let started = inspector.startTime(pid: owner) else { return true }
    guard let want = session.ownerPIDStart else { return false }
    return started != want
}

/// The dead members of `outcome` the app may mark failed itself: only those
/// whose owner process is gone too. A live owner finalizes its own session.
public func deadToMark(_ item: RunItem, outcome: StopOutcome, inspector: ProcessInspector) -> [String] {
    let dead = Set(outcome.dead)
    return item.sessions.filter { dead.contains($0.id) && ownerGone($0, inspector: inspector) }.map(\.id)
}
