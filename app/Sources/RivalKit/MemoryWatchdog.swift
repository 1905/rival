import Darwin
import Foundation
import os

/// What one watchdog tick concluded about the process footprint.
public enum WatchdogVerdict: Equatable, Sendable {
    /// Below the limit.
    case ok
    /// The footprint could not be read. Never a reason to quit.
    case unknown
    /// At or above the limit.
    case over(bytes: UInt64)
}

/// `.over` when `bytes` is at or above `limit`, `.unknown` when it is nil.
public func judgeFootprint(_ bytes: UInt64?, limit: UInt64) -> WatchdogVerdict {
    guard let bytes else { return .unknown }
    return bytes >= limit ? .over(bytes: bytes) : .ok
}

/// This process's `phys_footprint`: the number Activity Monitor and
/// `footprint(1)` show. Nil when the call fails.
///
/// Read through `proc_pid_rusage`, which returns the same counter as
/// `task_info(TASK_VM_INFO)` but needs no `mach_task_self_`, a C global that
/// Swift 6 strict concurrency rejects.
public func currentFootprint() -> UInt64? {
    var info = rusage_info_v4()
    let rc = withUnsafeMutablePointer(to: &info) { ptr in
        ptr.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) {
            proc_pid_rusage(getpid(), RUSAGE_INFO_V4, $0)
        }
    }
    guard rc == 0 else { return nil }
    return info.ri_phys_footprint
}

/// Quits the app when its memory footprint reaches `limit`.
///
/// - A `DispatchSourceTimer` on a global `.utility` queue reads the footprint
///   every `interval`. It never touches the main thread, so it still fires
///   when the main thread is stuck in a loop (the pre-release menu bar spinner bug, 2026-09-29).
/// - At or above the limit: one `Logger.fault`, one line appended to
///   `logURL`, then `exit(70)`. No crash dialog; the log line is the record.
/// - Fires at most once. A failed footprint read does nothing.
///
/// The reader and the exit hook are injected, so tests drive `check()`
/// without allocating a gigabyte or ending the test process.
public final class MemoryWatchdog: @unchecked Sendable {
    /// 1 GiB.
    public static let defaultLimit: UInt64 = 1 << 30
    /// `EX_SOFTWARE`: the exit code after a watchdog quit.
    public static let exitCode: Int32 = 70

    /// `~/Library/Logs/Rival/watchdog.log`.
    public static var defaultLogURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/Rival", isDirectory: true)
            .appendingPathComponent("watchdog.log")
    }

    public let limit: UInt64
    public let logURL: URL
    public let version: String

    private let interval: DispatchTimeInterval
    private let read: @Sendable () -> UInt64?
    private let exit: @Sendable (Int32) -> Void
    private let logger = Logger(subsystem: "dev.1905.rival", category: "watchdog")
    private let lock = NSLock()
    private var fired = false
    private var timer: DispatchSourceTimer?

    public init(limit: UInt64 = defaultLimit, interval: DispatchTimeInterval = .seconds(5),
                read: @escaping @Sendable () -> UInt64? = currentFootprint,
                logURL: URL = defaultLogURL,
                version: String,
                exit: @escaping @Sendable (Int32) -> Void = { Darwin.exit($0) }) {
        self.limit = limit
        self.interval = interval
        self.read = read
        self.logURL = logURL
        self.version = version
        self.exit = exit
    }

    /// Starts the timer. A second call does nothing.
    public func start() {
        lock.lock()
        defer { lock.unlock() }
        guard timer == nil else { return }
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .utility))
        t.schedule(deadline: .now() + interval, repeating: interval, leeway: .seconds(1))
        t.setEventHandler { [weak self] in self?.check() }
        t.resume()
        timer = t
    }

    /// One tick: read, judge, and on `.over` log and exit.
    public func check() {
        guard case .over(let bytes) = judgeFootprint(read(), limit: limit) else { return }
        lock.lock()
        let first = !fired
        fired = true
        lock.unlock()
        guard first else { return }
        logger.fault("footprint \(bytes, privacy: .public) >= limit \(self.limit, privacy: .public), version \(self.version, privacy: .public); quitting")
        appendLogLine(footprint: bytes)
        exit(Self.exitCode)
    }

    /// Appends `<ISO8601 local> footprint=<n> limit=<n> version=<v>`. Write
    /// errors are ignored: the `Logger.fault` above already recorded it.
    private func appendLogLine(footprint: UInt64) {
        let fmt = ISO8601DateFormatter()
        fmt.timeZone = .current
        let line = "\(fmt.string(from: Date())) footprint=\(footprint) limit=\(limit) version=\(version)\n"
        let fm = FileManager.default
        try? fm.createDirectory(at: logURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !fm.fileExists(atPath: logURL.path) {
            fm.createFile(atPath: logURL.path, contents: nil)
        }
        guard let handle = try? FileHandle(forWritingTo: logURL) else { return }
        defer { try? handle.close() }
        _ = try? handle.seekToEnd()
        try? handle.write(contentsOf: Data(line.utf8))
    }
}
