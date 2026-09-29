import Darwin
import Foundation

/// The run to stop after the user confirmed `confirmedIDs`, re-read from the
/// current snapshot (TUI `confirmKill`): a member that finished while the
/// sheet was open is dropped, so its possibly reused PID gets no signal.
/// Returns nil when nothing confirmed is still live.
public func confirmedStopTarget(_ current: RunItem, confirmedIDs: Set<String>) -> RunItem? {
    let members = stopCandidates(current).filter { confirmedIDs.contains($0.id) }
    return members.isEmpty ? nil : RunItem(id: current.id, sessions: members)
}

/// The confirmed stop, end to end: re-reads run `runID` from `runs` (the
/// current snapshot), signals what `stop` allows, marks the dead members nobody
/// else will finalize, and returns the toast.
///
/// Signalled members are left alone: their rival process handles SIGTERM and
/// finalizes its own session. Dead members are written only when their owner
/// is gone too (the reaper rule, `deadToMark`).
public func performStop(
    runID: String, confirmedIDs: Set<String>, runs: [RunItem], root: URL,
    inspector: ProcessInspector, signaller: Signaller
) -> String {
    guard let current = runs.first(where: { $0.id == runID }),
          let target = confirmedStopTarget(current, confirmedIDs: confirmedIDs)
    else { return "nothing running" }
    let outcome = stop(target, inspector: inspector, signaller: signaller)
    let marked = markStopped(sessionIDs: deadToMark(target, outcome: outcome, inspector: inspector), root: root)
    return stopToast(outcome, marked: marked.count)
}

/// Marks each session failed as already dead in `<root>/sessions/<id>.json`,
/// the way the TUI's `failSessionForKill(s, 1, …)` does: status "failed",
/// `exit_code` 1, `error` "killed (process already dead)", `end_time` and
/// `duration` (end − `start_time`, rounded to the second). Every other key is
/// kept as it was. The app never marks a signalled session: its rival process
/// finalizes it.
///
/// The file is re-read first. A session whose on-disk status is no longer
/// running or queued (its owner finished it meanwhile) is left alone. The
/// write goes through a unique `<id>.json.tmp-XXXXXX` file, then rename, like
/// Go's `Save`, so two writers never share a temp file. A missing or
/// unreadable file is skipped. Returns the ids written.
@discardableResult
public func markStopped(sessionIDs: [String], root: URL, now: Date = Date()) -> [String] {
    let dir = RivalPaths.sessionsDir(root: root)
    var written: [String] = []
    for id in sessionIDs where !id.isEmpty && !id.contains("/") {
        let url = dir.appendingPathComponent(id + ".json")
        guard let data = FileManager.default.contents(atPath: url.path),
              var obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              isLive(obj["status"] as? String ?? "")
        else { continue }

        obj["status"] = "failed"
        obj["exit_code"] = 1
        obj["error"] = "killed (process already dead)"
        obj["end_time"] = formatRFC3339(now)
        if let start = (obj["start_time"] as? String).flatMap(parseRFC3339), !start.isGoZero {
            let secs = Int(now.timeIntervalSince(start).rounded(.toNearestOrAwayFromZero))
            obj["duration"] = formatGoDuration(seconds: secs)
        }

        guard let out = try? JSONSerialization.data(
            withJSONObject: obj, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        ), let tmp = writeUniqueTemp(out, dir: dir, prefix: id + ".json.tmp-")
        else { continue }
        if rename(tmp, url.path) != 0 {
            try? FileManager.default.removeItem(atPath: tmp)
            continue
        }
        written.append(id)
    }
    return written
}

/// Writes `data` to a new `<dir>/<prefix>XXXXXX` file (mode 0600, from
/// `mkstemp`) and returns its path, or nil on failure.
private func writeUniqueTemp(_ data: Data, dir: URL, prefix: String) -> String? {
    var template = Array(dir.appendingPathComponent(prefix + "XXXXXX").path.utf8CString)
    let fd = mkstemp(&template)
    guard fd >= 0 else { return nil }
    let path = String(cString: template)
    let ok = data.withUnsafeBytes { buf -> Bool in
        var off = 0
        while off < buf.count {
            let n = write(fd, buf.baseAddress! + off, buf.count - off)
            if n <= 0 { return false }
            off += n
        }
        return true
    }
    if close(fd) != 0 || !ok {
        try? FileManager.default.removeItem(atPath: path)
        return nil
    }
    return path
}

/// RFC 3339 with milliseconds in UTC, which Go's `time.Time` JSON decoding
/// and `parseRFC3339` both accept.
func formatRFC3339(_ date: Date) -> String {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    f.timeZone = TimeZone(identifier: "UTC")
    return f.string(from: date)
}
