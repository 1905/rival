import Darwin
import Foundation
import Observation

/// Where rival keeps its state.
public enum RivalPaths {
    /// `RIVAL_HOME` when set and non-empty, else `~/.rival`. The Go CLI always
    /// writes under `~/.rival`; the override exists so the app can run against
    /// a fixture directory.
    public static func root(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL {
        if let home = environment["RIVAL_HOME"], !home.isEmpty {
            return URL(fileURLWithPath: (home as NSString).expandingTildeInPath, isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".rival", isDirectory: true)
    }

    /// `<root>/sessions`.
    public static func sessionsDir(root: URL) -> URL {
        root.appendingPathComponent("sessions", isDirectory: true)
    }
}

/// How far the initial scan got: `done` of `total` session files read.
public struct LoadProgress: Equatable, Sendable {
    public var done: Int
    public var total: Int

    public init(done: Int, total: Int) {
        self.done = done
        self.total = total
    }
}

/// Live, grouped view of `<root>/sessions`.
///
/// - A `DispatchSource` on the sessions directory reacts to file creates,
///   renames and deletes (the CLI saves through tmp + rename). A poll every
///   `pollInterval` covers what the directory vnode does not report, and
///   re-arms the watcher.
/// - While the sessions directory is missing, the watcher sits on `root`
///   instead, and the poll covers a missing `root`.
/// - Refreshes are debounced: the first event starts a `debounce` timer and
///   later events join it, so a burst of writes costs one scan.
/// - A scan re-decodes only files whose size or mtime changed. Temp files
///   (`.json.tmp`, `.json.tmp-*`) are ignored. A file that fails to decode
///   is skipped (its last good copy stays) and retried on the next scan.
/// - Snapshots drop the full prompt; `fullSession(id:)` reads it.
///
/// Call `stop()` before releasing the store; it closes the directory handle.
@MainActor @Observable
public final class SessionStore {
    public let root: URL
    public let sessionsDir: URL
    /// Grouped runs, newest first (by the first-seen member's start time).
    public private(set) var runs: [RunItem] = []
    /// Session counts per status across `runs`, computed once per snapshot.
    public private(set) var stats = SessionStats()
    /// Increments each time a scan publishes a changed snapshot. The first
    /// scan after `start()` always publishes.
    public private(set) var revision = 0
    /// False while `<root>/sessions` does not exist (the empty state).
    public private(set) var directoryExists = false
    /// True until the first scan publishes. The views show a loader and "…"
    /// counts meanwhile, never a misleading 0.
    public private(set) var isLoading = true
    /// The initial scan's progress, updated every `SessionScanner.progressEvery`
    /// files while `isLoading`. Ends at total/total.
    public private(set) var loadProgress = LoadProgress(done: 0, total: 0)

    @ObservationIgnored private let debounce: Duration
    @ObservationIgnored private let pollInterval: Duration
    @ObservationIgnored private let scanner: SessionScanner
    @ObservationIgnored private var appliedGeneration = 0
    @ObservationIgnored private var active = false
    @ObservationIgnored private var pendingRefresh: Task<Void, Never>?
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    @ObservationIgnored private var watcher: DirectoryWatcher?

    public init(root: URL, debounce: Duration = .milliseconds(250), pollInterval: Duration = .seconds(2)) {
        self.root = root
        self.sessionsDir = RivalPaths.sessionsDir(root: root)
        self.debounce = debounce
        self.pollInterval = pollInterval
        self.scanner = SessionScanner(dir: sessionsDir)
    }

    public func start() {
        guard !active else { return }
        active = true
        rearmWatcher()
        Task { await refresh() }
        pollTask = Task { [weak self, pollInterval] in
            while !Task.isCancelled {
                try? await Task.sleep(for: pollInterval)
                guard let self, !Task.isCancelled else { return }
                await self.refresh()
            }
        }
    }

    public func stop() {
        active = false
        pollTask?.cancel()
        pollTask = nil
        pendingRefresh?.cancel()
        pendingRefresh = nil
        watcher?.cancel()
        watcher = nil
    }

    /// Reads one session with its full prompt off the main actor, or nil when
    /// the file is gone or unreadable.
    public nonisolated func fullSession(id: String) async -> Session? {
        let url = sessionsDir.appendingPathComponent(id + ".json")
        return await Task.detached(priority: .userInitiated) { try? Session.load(from: url) }.value
    }

    private func scheduleRefresh() {
        guard active, pendingRefresh == nil else { return }
        pendingRefresh = Task { [weak self, debounce] in
            try? await Task.sleep(for: debounce)
            guard let self, !Task.isCancelled else { return }
            self.pendingRefresh = nil
            await self.refresh()
        }
    }

    private func refresh() async {
        var report: (@Sendable (Int, Int) -> Void)?
        if isLoading {
            report = { [weak self] done, total in
                Task { @MainActor in self?.noteProgress(done: done, total: total) }
            }
        }
        let result = await scanner.scan(progress: report)
        guard active else { return }
        // Scans can finish out of order across suspension points; never let an
        // older one overwrite a newer snapshot.
        if result.generation > appliedGeneration {
            appliedGeneration = result.generation
            if result.changed {
                runs = groupRuns(result.sessions)
                stats = sessionStats(runs)
                revision += 1
            }
            if directoryExists != result.directoryExists { directoryExists = result.directoryExists }
            if isLoading {
                // Progress hops arrive as separate main-actor tasks and may
                // land after this; noteProgress drops them once loaded.
                loadProgress = LoadProgress(done: result.fileCount, total: result.fileCount)
                isLoading = false
                // The first scan decodes every session file and leaves ~15 MB
                // of freed malloc pages behind (measured on 6100 sessions).
                // Hand them back once; later scans only touch changed files.
                Task.detached(priority: .utility) { _ = malloc_zone_pressure_relief(nil, 0) }
            }
        }
        rearmWatcher()
    }

    private func noteProgress(done: Int, total: Int) {
        guard isLoading, done > loadProgress.done || total != loadProgress.total else { return }
        loadProgress = LoadProgress(done: done, total: total)
    }

    /// Watches the sessions dir, or `root` while it is missing. Re-opens the
    /// watch when the directory was replaced (new inode) or appeared.
    private func rearmWatcher() {
        guard active else { return }
        let target = [sessionsDir.path, root.path].first { inode(of: $0) != nil }
        guard let target, let ino = inode(of: target) else {
            watcher?.cancel()
            watcher = nil
            return
        }
        if let w = watcher, w.path == target, w.inode == ino { return }
        watcher?.cancel()
        watcher = DirectoryWatcher(path: target, inode: ino) { [weak self] in
            Task { @MainActor in
                // A change under root may be the sessions dir appearing.
                self?.rearmWatcher()
                self?.scheduleRefresh()
            }
        }
    }
}

private func inode(of path: String) -> UInt64? {
    var st = stat()
    guard stat(path, &st) == 0, (st.st_mode & S_IFMT) == S_IFDIR else { return nil }
    return UInt64(st.st_ino)
}

/// A kqueue vnode watch on one directory.
private final class DirectoryWatcher {
    let path: String
    let inode: UInt64
    private let source: DispatchSourceFileSystemObject?

    init(path: String, inode: UInt64, onEvent: @escaping @Sendable () -> Void) {
        self.path = path
        self.inode = inode
        let fd = open(path, O_EVTONLY)
        guard fd >= 0 else {
            source = nil
            return
        }
        let src = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd,
            eventMask: [.write, .delete, .rename, .link, .attrib, .extend],
            queue: DispatchQueue.global(qos: .utility)
        )
        src.setEventHandler(handler: onEvent)
        src.setCancelHandler { close(fd) }
        src.resume()
        source = src
    }

    func cancel() {
        source?.cancel()
    }
}

struct ScanResult: Sendable {
    let generation: Int
    let changed: Bool
    let directoryExists: Bool
    let sessions: [Session]
    /// Session files seen in the directory, decodable or not.
    var fileCount = 0
}

/// The mtime+size cache behind `SessionStore` (the same idea as
/// `sessionview.Cache`). An actor, so file I/O stays off the main thread.
actor SessionScanner {
    private struct Entry {
        let size: Int64
        let mtimeSec: Int
        let mtimeNsec: Int
        let session: Session
    }

    /// Files between two progress reports.
    static let progressEvery = 100

    private let dir: URL
    private var files: [String: Entry] = [:]
    private var generation = 0
    private var scannedOnce = false

    init(dir: URL) {
        self.dir = dir
    }

    /// One pass over the directory. Changed files are decoded concurrently
    /// (`concurrentPerform` is synchronous, so the actor is never re-entered
    /// mid-scan). `progress`, when set, gets (done, total) every
    /// `progressEvery` files and once at the end, from worker threads.
    func scan(progress: (@Sendable (Int, Int) -> Void)? = nil) -> ScanResult {
        generation += 1
        var changed = !scannedOnce
        scannedOnce = true

        guard let names = try? FileManager.default.contentsOfDirectory(atPath: dir.path) else {
            if !files.isEmpty {
                files.removeAll()
                changed = true
            }
            return ScanResult(generation: generation, changed: changed, directoryExists: false, sessions: [])
        }

        var seen = Set<String>()
        var todo: [Pending] = []
        // Temp files (`<id>.json.tmp`, `<id>.json.tmp-*`) never end in ".json";
        // the second check keeps that true for any future temp suffix.
        for name in names where name.hasSuffix(".json") && !name.contains(".json.tmp") {
            let path = dir.appendingPathComponent(name).path
            var st = stat()
            guard stat(path, &st) == 0, (st.st_mode & S_IFMT) == S_IFREG else { continue }
            seen.insert(name)
            let size = Int64(st.st_size)
            let sec = st.st_mtimespec.tv_sec, nsec = st.st_mtimespec.tv_nsec
            if let e = files[name], e.size == size, e.mtimeSec == sec, e.mtimeNsec == nsec { continue }
            todo.append(Pending(name: name, path: path, size: size, mtimeSec: sec, mtimeNsec: nsec))
        }

        let decoded = Self.decode(todo, cached: seen.count - todo.count, total: seen.count, progress: progress)
        for (p, session) in zip(todo, decoded) {
            // Corrupt or mid-write: keep the last good copy, if any. Its
            // stale size/mtime makes the next scan retry this file.
            guard let session else { continue }
            files[p.name] = Entry(size: p.size, mtimeSec: p.mtimeSec, mtimeNsec: p.mtimeNsec, session: session)
            changed = true
        }
        for name in files.keys where !seen.contains(name) {
            files.removeValue(forKey: name)
            changed = true
        }

        // Unchanged: the store ignores `sessions`, so skip the copy and sort
        // of every session (6000+ on a real ~/.rival) on each 2s poll.
        let sessions = !changed ? [] : files.values.map(\.session).sorted {
            $0.startTime != $1.startTime ? $0.startTime > $1.startTime : $0.id < $1.id
        }
        return ScanResult(generation: generation, changed: changed, directoryExists: true,
                          sessions: sessions, fileCount: seen.count)
    }

    private struct Pending: Sendable {
        let name: String
        let path: String
        let size: Int64
        let mtimeSec: Int
        let mtimeNsec: Int
    }

    /// Decode results by index plus the shared progress counter.
    private final class Slots: @unchecked Sendable {
        let lock = NSLock()
        var items: [Session?]
        var done: Int

        init(count: Int, done: Int) {
            items = Array(repeating: nil, count: count)
            self.done = done
        }
    }

    /// Decodes `todo` in parallel chunks, one decoder per chunk.
    private static func decode(_ todo: [Pending], cached: Int, total: Int,
                               progress: (@Sendable (Int, Int) -> Void)?) -> [Session?] {
        let slots = Slots(count: todo.count, done: cached)
        if !todo.isEmpty {
            let chunks = min(todo.count, ProcessInfo.processInfo.activeProcessorCount * 4)
            DispatchQueue.concurrentPerform(iterations: chunks) { chunk in
                let decoder = JSONDecoder()
                decoder.userInfo[Session.skipPromptKey] = true
                var i = chunk
                while i < todo.count {
                    let s = SessionSummary.load(path: todo[i].path, size: todo[i].size, decoder: decoder)
                    slots.lock.lock()
                    slots.items[i] = s
                    slots.done += 1
                    let done = slots.done
                    slots.lock.unlock()
                    if done % progressEvery == 0 { progress?(done, total) }
                    i += chunks
                }
            }
        }
        if total > 0 { progress?(total, total) }
        return slots.items
    }
}
