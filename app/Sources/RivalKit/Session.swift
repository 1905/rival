import Foundation

/// One review run as the Go CLI stores it in `<root>/sessions/<id>.json`
/// (`rival/internal/session/session.go`, struct `Session`).
///
/// Decoding is tolerant: unknown keys are ignored, and a missing or mistyped
/// key takes its default. Only `id` is required, because a record without one
/// cannot be selected or grouped. Go's `omitempty` fields map to optionals, and
/// an empty string or a zero `pid_start` decodes as `nil`, the same meaning Go
/// gives them.
public struct Session: Decodable, Identifiable, Hashable, Sendable {
    public let id: String
    public let groupID: String?
    public let cli: String
    public let mode: String
    public let model: String
    public let effort: String
    public let reviewScope: String?
    /// The full prompt. `SessionStore` drops it from list snapshots to keep
    /// memory flat (the Go summaries do the same); load it with
    /// `Session.load(from:)` for the Prompt tab.
    public let prompt: String?
    public let promptPreview: String?
    public let status: String
    /// Go writes the zero time `0001-01-01T00:00:00Z` when unset. Check it with
    /// `Date.isGoZero`, never against `nil`.
    public let startTime: Date
    public let queuedAt: Date?
    public let queuePosition: Int?
    public let endTime: Date?
    public let exitCode: Int?
    public let duration: String?
    public let workDir: String
    public let logFile: String
    public let outputBytes: Int64
    public let outputLines: Int
    public let error: String?
    /// The provider account the run used, when rival recorded one.
    public let account: String?
    public let pid: Int32
    /// Process start time of `pid` in Unix nanoseconds (Go `procinfo.StartNanos`).
    public let pidStart: Int64?
    /// The rival process that drives this run (Go `OwnerPID`). It finalizes
    /// the session itself; nil for records from older releases.
    public let ownerPID: Int32?
    /// Start time of `ownerPID` in Unix nanoseconds.
    public let ownerPIDStart: Int64?

    public init(
        id: String,
        groupID: String? = nil,
        cli: String = "",
        mode: String = "",
        model: String = "",
        effort: String = "",
        reviewScope: String? = nil,
        prompt: String? = nil,
        promptPreview: String? = nil,
        status: String = "",
        startTime: Date = .goZero,
        queuedAt: Date? = nil,
        queuePosition: Int? = nil,
        endTime: Date? = nil,
        exitCode: Int? = nil,
        duration: String? = nil,
        workDir: String = "",
        logFile: String = "",
        outputBytes: Int64 = 0,
        outputLines: Int = 0,
        error: String? = nil,
        account: String? = nil,
        pid: Int32 = 0,
        pidStart: Int64? = nil,
        ownerPID: Int32? = nil,
        ownerPIDStart: Int64? = nil
    ) {
        self.id = id
        self.groupID = groupID.nonEmpty
        self.cli = cli
        self.mode = mode
        self.model = model
        self.effort = effort
        self.reviewScope = reviewScope.nonEmpty
        self.prompt = prompt.nonEmpty
        self.promptPreview = promptPreview.nonEmpty
        self.status = status
        self.startTime = startTime
        self.queuedAt = queuedAt
        self.queuePosition = queuePosition
        self.endTime = endTime
        self.exitCode = exitCode
        self.duration = duration.nonEmpty
        self.workDir = workDir
        self.logFile = logFile
        self.outputBytes = outputBytes
        self.outputLines = outputLines
        self.error = error.nonEmpty
        self.account = account.nonEmpty
        self.pid = pid
        self.pidStart = (pidStart ?? 0) == 0 ? nil : pidStart
        self.ownerPID = (ownerPID ?? 0) == 0 ? nil : ownerPID
        self.ownerPIDStart = (ownerPIDStart ?? 0) == 0 ? nil : ownerPIDStart
    }

    enum CodingKeys: String, CodingKey {
        case id, cli, mode, model, effort, prompt, status, duration, error, account, pid
        case groupID = "group_id"
        case reviewScope = "review_scope"
        case promptPreview = "prompt_preview"
        case startTime = "start_time"
        case queuedAt = "queued_at"
        case queuePosition = "queue_position"
        case endTime = "end_time"
        case exitCode = "exit_code"
        case workDir = "work_dir"
        case logFile = "log_file"
        case outputBytes = "output_bytes"
        case outputLines = "output_lines"
        case pidStart = "pid_start"
        case ownerPID = "owner_pid"
        case ownerPIDStart = "owner_pid_start"
    }

    /// Set this `userInfo` key to `true` on a `JSONDecoder` to drop the prompt
    /// after decoding. The list only needs `prompt_preview`.
    public static let skipPromptKey = CodingUserInfoKey(rawValue: "rival.skipPrompt")!

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func str(_ k: CodingKeys) -> String? { (try? c.decodeIfPresent(String.self, forKey: k)) ?? nil }
        func int(_ k: CodingKeys) -> Int? { (try? c.decodeIfPresent(Int.self, forKey: k)) ?? nil }
        func i64(_ k: CodingKeys) -> Int64? { (try? c.decodeIfPresent(Int64.self, forKey: k)) ?? nil }
        func date(_ k: CodingKeys) -> Date? { str(k).flatMap(parseRFC3339) }

        let id = try c.decode(String.self, forKey: .id)
        let skipPrompt = decoder.userInfo[Session.skipPromptKey] as? Bool ?? false
        let pid = int(.pid) ?? 0
        self.init(
            id: id,
            groupID: str(.groupID),
            cli: str(.cli) ?? "",
            mode: str(.mode) ?? "",
            model: str(.model) ?? "",
            effort: str(.effort) ?? "",
            reviewScope: str(.reviewScope),
            prompt: skipPrompt ? nil : str(.prompt),
            promptPreview: str(.promptPreview),
            status: str(.status) ?? "",
            startTime: date(.startTime) ?? .goZero,
            queuedAt: date(.queuedAt),
            queuePosition: int(.queuePosition),
            endTime: date(.endTime),
            exitCode: int(.exitCode),
            duration: str(.duration),
            workDir: str(.workDir) ?? "",
            logFile: str(.logFile) ?? "",
            outputBytes: i64(.outputBytes) ?? 0,
            outputLines: int(.outputLines) ?? 0,
            error: str(.error),
            account: str(.account),
            pid: Int32(clamping: pid),
            pidStart: i64(.pidStart),
            ownerPID: int(.ownerPID).map { Int32(clamping: $0) },
            ownerPIDStart: i64(.ownerPIDStart)
        )
    }

    /// Decodes one session file with its full prompt.
    public static func load(from url: URL) throws -> Session {
        try JSONDecoder().decode(Session.self, from: Data(contentsOf: url))
    }
}

extension Optional where Wrapped == String {
    fileprivate var nonEmpty: String? {
        guard let s = self, !s.isEmpty else { return nil }
        return s
    }
}

extension Date {
    /// Go's zero `time.Time`: 0001-01-01T00:00:00Z.
    public static let goZero = Date(timeIntervalSince1970: -62_135_596_800)

    /// True for Go's zero time (or anything before it), the "unset" value.
    public var isGoZero: Bool { self <= Date.goZero }
}

/// Parses Go's `time.Time` JSON form (RFC 3339 with 0-9 fractional digits and
/// a `Z` or `±hh:mm` offset). The calendar math is done by hand so year 1 (Go's
/// zero time) and nanosecond fractions round-trip without a formatter.
public func parseRFC3339(_ s: String) -> Date? {
    let b = Array(s.utf8)
    func num(_ from: Int, _ len: Int) -> Int? {
        guard from + len <= b.count else { return nil }
        var v = 0
        for i in from..<(from + len) {
            let d = Int(b[i]) - 48
            guard (0...9).contains(d) else { return nil }
            v = v * 10 + d
        }
        return v
    }
    func at(_ i: Int, _ ch: UInt8) -> Bool { i < b.count && b[i] == ch }

    guard let year = num(0, 4), at(4, 45), let month = num(5, 2), at(7, 45), let day = num(8, 2),
          b.count > 10, b[10] == 84 || b[10] == 116 || b[10] == 32,
          let hour = num(11, 2), at(13, 58), let minute = num(14, 2), at(16, 58), let second = num(17, 2),
          (1...12).contains(month), (1...31).contains(day), hour < 24, minute < 60, second < 61
    else { return nil }

    var i = 19
    var fraction = 0.0
    if at(i, 46) {
        i += 1
        var scale = 0.1
        let start = i
        while i < b.count, (48...57).contains(b[i]) {
            fraction += Double(b[i] - 48) * scale
            scale /= 10
            i += 1
        }
        guard i > start else { return nil }
    }

    var offset = 0
    if at(i, 90) || at(i, 122) {
        i += 1
    } else if at(i, 43) || at(i, 45) {
        let sign = b[i] == 45 ? -1 : 1
        guard let oh = num(i + 1, 2), at(i + 3, 58), let om = num(i + 4, 2) else { return nil }
        offset = sign * (oh * 3600 + om * 60)
        i += 6
    } else {
        return nil
    }
    guard i == b.count else { return nil }

    let days = daysFromCivil(year: year, month: month, day: day)
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset
    return Date(timeIntervalSince1970: Double(secs) + fraction)
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar (H. Hinnant).
func daysFromCivil(year: Int, month: Int, day: Int) -> Int {
    let y = month <= 2 ? year - 1 : year
    let era = (y >= 0 ? y : y - 399) / 400
    let yoe = y - era * 400
    let mp = (month + 9) % 12
    let doy = (153 * mp + 2) / 5 + day - 1
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
    return era * 146_097 + doe - 719_468
}
