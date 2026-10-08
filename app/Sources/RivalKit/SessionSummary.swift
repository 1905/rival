import Foundation

/// Reads one session file for the list, without its prompt: the Swift twin of
/// the rival CLI's summary loader.
///
/// The CLI writes indented JSON with one field per line, and the prompt is the one
/// long line. So a file over `lineScanMin` is not JSON-decoded whole: the known
/// metadata lines are picked out and only they are decoded, which skips
/// parsing the prompt. Over 2 × `edgeBytes` only the first and last
/// `edgeBytes` are even read (the CLI does the same). On real data (~315 MB of
/// session JSON, avg 105 KB) this is most of the startup cost. A file whose
/// lines hold no usable metadata is decoded whole, as before.
enum SessionSummary {
    static let edgeBytes = 64 << 10
    /// Below this a whole decode is as cheap as picking lines.
    static let lineScanMin: Int64 = 4 << 10

    /// Every field the list decodes. `prompt` is deliberately absent.
    static let fields: Set<String> = [
        "id", "group_id", "cli", "mode", "model", "effort", "review_scope",
        "prompt_preview", "prompt_hash", "status", "start_time", "queued_at",
        "queue_position", "end_time", "exit_code", "duration", "work_dir",
        "log_file", "output_bytes", "output_lines", "error", "account", "pid",
        "pid_start", "owner_pid", "owner_pid_start",
    ]

    /// `decoder` must set `Session.skipPromptKey`.
    static func load(path: String, size: Int64, decoder: JSONDecoder) -> Session? {
        if size > Int64(2 * edgeBytes), let edges = readEdges(path: path),
           let s = decodeLines(edges, decoder: decoder) {
            return s
        }
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        if size > lineScanMin, let s = decodeLines([data], decoder: decoder) {
            return s
        }
        return try? decoder.decode(Session.self, from: data)
    }

    private static func readEdges(path: String) -> [Data]? {
        guard let fh = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? fh.close() }
        guard let prefix = try? fh.read(upToCount: edgeBytes),
              let end = try? fh.seekToEnd(), end >= UInt64(edgeBytes)
        else { return nil }
        try? fh.seek(toOffset: end - UInt64(edgeBytes))
        guard let suffix = try? fh.read(upToCount: edgeBytes) else { return nil }
        return [prefix, suffix]
    }

    /// Decodes the metadata lines of `chunks` as one object, or nil when they
    /// hold no `id`.
    private static func decodeLines(_ chunks: [Data], decoder: JSONDecoder) -> Session? {
        var raw: [String: Data] = [:]
        for c in chunks { collect(c, into: &raw) }
        guard raw["id"] != nil else { return nil }

        var obj = Data("{".utf8)
        for (i, (k, v)) in raw.enumerated() {
            if i > 0 { obj.append(UInt8(ascii: ",")) }
            obj.append(contentsOf: Data(jsonString(k).utf8))
            obj.append(UInt8(ascii: ":"))
            obj.append(v)
        }
        obj.append(UInt8(ascii: "}"))
        return try? decoder.decode(Session.self, from: obj)
    }

    /// Picks `"key": value,` lines whose key is in `fields` and whose value is
    /// valid JSON. A line cut by an edge fails one of the checks. Later
    /// matches (the suffix) win, as in the CLI. Lines are found with `memchr`, so
    /// the long prompt line costs one scan and no parsing.
    private static func collect(_ data: Data, into raw: inout [String: Data]) {
        data.withUnsafeBytes { (buf: UnsafeRawBufferPointer) in
            guard let base = buf.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return }
            let n = buf.count
            var start = 0
            while start < n {
                let nl = memchr(base + start, 0x0A, n - start)
                let end = nl.map { UnsafePointer<UInt8>($0.assumingMemoryBound(to: UInt8.self)) - base } ?? n
                defer { start = end + 1 }
                var lo = start, hi = end
                while lo < hi, isSpace(base[lo]) { lo += 1 }
                // Keys are short: a line whose key does not close within 40
                // bytes (the prompt, a cut line) is skipped unread.
                guard hi - lo >= 4, base[lo] == UInt8(ascii: "\"") else { continue }
                var q = lo + 1
                while q < min(hi, lo + 40), base[q] != UInt8(ascii: "\""), base[q] != UInt8(ascii: "\\") { q += 1 }
                guard q < hi, base[q] == UInt8(ascii: "\""), q + 1 < hi, base[q + 1] == UInt8(ascii: ":"),
                      let key = String(bytes: UnsafeBufferPointer(start: base + lo + 1, count: q - lo - 1), encoding: .utf8),
                      fields.contains(key)
                else { continue }
                var vlo = q + 2
                while vlo < hi, isSpace(base[vlo]) { vlo += 1 }
                while hi > vlo, isSpace(base[hi - 1]) { hi -= 1 }
                if hi > vlo, base[hi - 1] == UInt8(ascii: ",") { hi -= 1 }
                while hi > vlo, isSpace(base[hi - 1]) { hi -= 1 }
                guard hi > vlo else { continue }
                let v = Data(bytes: base + vlo, count: hi - vlo)
                guard (try? JSONSerialization.jsonObject(with: v, options: .fragmentsAllowed)) != nil else { continue }
                raw[key] = v
            }
        }
    }

    private static func isSpace(_ b: UInt8) -> Bool {
        b == 0x20 || b == 0x09 || b == 0x0D || b == 0x0A
    }

    /// Summary keys are plain ASCII identifiers; no escaping needed.
    private static func jsonString(_ k: String) -> String { "\"" + k + "\"" }
}
