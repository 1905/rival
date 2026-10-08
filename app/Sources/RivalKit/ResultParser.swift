import Foundation

// A port of the rival CLI's answer extraction (review parsing, plan parsing
// and review formatting), so the app and `rival` agree on what a run's
// answer is.

/// One finding of a reviewer or plan payload (`review.ReviewerFinding`).
/// Missing or null strings decode as "", missing or null ints as 0. A value of
/// the wrong type is a decode error, as in the rival CLI.
public struct Finding: Decodable, Equatable, Sendable {
    public var file: String
    public var line: Int
    public var severity: String
    public var category: String
    public var title: String
    public var body: String
    /// nil when the payload has none or it is empty.
    public var failureScenario: String?
    /// nil when the payload has none or it is empty.
    public var suggestion: String?
    public var confidence: Int

    public init(file: String = "", line: Int = 0, severity: String = "", category: String = "",
                title: String = "", body: String = "", failureScenario: String? = nil,
                suggestion: String? = nil, confidence: Int = 0) {
        self.file = file
        self.line = line
        self.severity = severity
        self.category = category
        self.title = title
        self.body = body
        self.failureScenario = failureScenario
        self.suggestion = suggestion
        self.confidence = confidence
    }

    enum CodingKeys: String, CodingKey {
        case file, line, severity, category, title, body, suggestion, confidence
        case failureScenario = "failure_scenario"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func str(_ k: CodingKeys) throws -> String { try c.decodeIfPresent(String.self, forKey: k) ?? "" }
        func int(_ k: CodingKeys) throws -> Int { try c.decodeIfPresent(Int.self, forKey: k) ?? 0 }
        func opt(_ k: CodingKeys) throws -> String? {
            let s = try str(k)
            return s.isEmpty ? nil : s
        }
        file = try str(.file)
        line = try int(.line)
        severity = try str(.severity)
        category = try str(.category)
        title = try str(.title)
        body = try str(.body)
        failureScenario = try opt(.failureScenario)
        suggestion = try opt(.suggestion)
        confidence = try int(.confidence)
    }
}

/// What the Result tab shows for a finished run.
public enum RunResult: Equatable, Sendable {
    /// A JSON payload. `rating` is set for plan payloads, nil for
    /// a code review. `findings` are placeholder-free and sorted.
    case findings(summary: String, rating: Int?, findings: [Finding])
    /// A prose answer, sanitized for display.
    case markdown(String)
    /// No usable answer; the reason.
    case failed(String)
}

/// The canonical severity ladder, most severe first (`review.severities`).
public let severityNames = ["critical", "high", "medium", "low"]

/// Critical 0, high 1, medium 2, low 3, anything else 4. Case-insensitive
/// (`review.severityRank`).
public func severityRank(_ s: String) -> Int {
    severityNames.firstIndex(of: s.lowercased()) ?? severityNames.count
}

/// `f` ordered by severity (critical first), then confidence (highest first).
/// Stable: equal findings keep the model's order (`review.sortedFindings`).
public func sortedFindings(_ f: [Finding]) -> [Finding] {
    f.enumerated().sorted { a, b in
        let ra = severityRank(a.element.severity), rb = severityRank(b.element.severity)
        if ra != rb { return ra < rb }
        if a.element.confidence != b.element.confidence { return a.element.confidence > b.element.confidence }
        return a.offset < b.offset
    }.map(\.element)
}

// MARK: - Final answer

/// The part of a provider log that holds the model's final answer
/// (`review.FinalAnswer`, plus codex's footer and echo).
///
/// - No line that is exactly "codex": the whole text.
/// - Else the lines after the last such line. Codex streams the answer there,
///   then prints it again, clean, and puts a "tokens used" + count footer and
///   `hook: …` status lines somewhere among them (stdout and stderr share the
///   log, so the footer can land inside the second copy). The footer and the
///   hook lines are dropped. When the rest is the same block twice, one copy
///   is the answer; else all of it is.
public func finalAnswer(_ raw: String) -> String {
    let lines = raw.split(separator: "\n", omittingEmptySubsequences: false)
    guard let header = lines.lastIndex(where: { $0 == "codex" }) else { return raw }

    var kept: [Substring] = []
    var footerSeen = false
    var i = header + 1
    while i < lines.endIndex {
        let line = lines[i]
        let t = line.trimmingCharacters(in: .whitespaces)
        if !footerSeen, t == "tokens used", i + 1 < lines.endIndex, isTokenCount(lines[i + 1]) {
            footerSeen = true
            i += 2
            continue
        }
        if !footerSeen, t.hasPrefix("tokens used "), isTokenCount(t.dropFirst("tokens used ".count)) {
            footerSeen = true
            i += 1
            continue
        }
        if !isHookLine(line) { kept.append(line) }
        i += 1
    }

    var body = kept[...]
    while let f = body.first, f.allSatisfy(\.isWhitespace) { body = body.dropFirst() }
    while let l = body.last, l.allSatisfy(\.isWhitespace) { body = body.dropLast() }
    let half = body.count / 2
    if half > 0, body.count % 2 == 0, body.prefix(half).elementsEqual(body.suffix(half)) {
        body = body.prefix(half)
    }
    return body.joined(separator: "\n")
}

/// A token count: digits with "," or "." grouping ("78,402", "117.735").
private func isTokenCount<S: StringProtocol>(_ s: S) -> Bool {
    let t = s.trimmingCharacters(in: .whitespaces)
    guard let first = t.unicodeScalars.first, ("0"..."9").contains(first) else { return false }
    return t.unicodeScalars.allSatisfy { ("0"..."9").contains($0) || $0 == "," || $0 == "." }
}

/// Codex's hook status lines: "hook: Stop", "hook: PreToolUse Completed".
private func isHookLine(_ line: Substring) -> Bool {
    guard line.hasPrefix("hook: ") else { return false }
    let words = line.dropFirst("hook: ".count).split(separator: " ", omittingEmptySubsequences: false)
    guard let name = words.first, !name.isEmpty, name.allSatisfy({ $0.isASCII && $0.isLetter }) else { return false }
    return words.count == 1 || (words.count == 2 && words[1] == "Completed")
}

// MARK: - JSON objects

/// Every balanced, valid JSON object in `s`, in closing-brace order, including
/// objects nested inside larger non-JSON brace spans (`review.jsonObjects`).
///
/// One forward pass over the UTF-8 bytes with a stack of open-brace positions.
/// Outside any object only "{" matters, so stray quotes in prose can't desync
/// the scan. String and escape state is tracked only inside an object. Every
/// closing brace tests the span it closes for validity.
public func jsonObjects(_ s: String) -> [Substring] {
    var out: [Substring] = []
    var stack: [String.Index] = []
    var inStr = false
    var esc = false
    let utf8 = s.utf8
    var i = utf8.startIndex
    while i < utf8.endIndex {
        let c = utf8[i]
        defer { i = utf8.index(after: i) }
        if stack.isEmpty {
            if c == UInt8(ascii: "{") {
                stack.append(i)
                inStr = false
                esc = false
            }
            continue
        }
        if esc {
            esc = false
            continue
        }
        switch c {
        case UInt8(ascii: "\\"):
            if inStr { esc = true }
        case UInt8(ascii: "\""):
            inStr.toggle()
        case UInt8(ascii: "{") where !inStr:
            stack.append(i)
        case UInt8(ascii: "}") where !inStr:
            let start = stack.removeLast()
            let candidate = s[start...i]
            if isValidJSON(candidate) { out.append(candidate) }
        default:
            break
        }
    }
    return out
}

private func isValidJSON(_ s: Substring) -> Bool {
    (try? JSONSerialization.jsonObject(with: Data(s.utf8), options: [.fragmentsAllowed])) != nil
}

// MARK: - Result

/// The schema example summaries from the prompt contracts. A real answer never
/// carries them verbatim (`isExampleSummary`, `planExampleSummary`).
let reviewerExampleSummary = "1-3 sentence reviewer summary"
let planExampleSummary = "1-3 sentence overall assessment of the plan"

/// A finding copied from a schema example (`review.isPlaceholderFinding`).
/// Exact literals only: real dual categories like "bug|security" stay.
func isPlaceholderFinding(_ f: Finding) -> Bool {
    switch f.category {
    case "bug|security|performance|concurrency|architecture|tests|ux",
         "bug|gap|ambiguity|scope|verification",
         "reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni":
        return true
    default:
        return f.file == "path/to/file" || f.severity == "critical|high|medium|low"
    }
}

private struct Payload: Decodable {
    var summary: String
    var rating: Int
    var findings: [Finding]

    enum CodingKeys: String, CodingKey { case summary, rating, findings }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        rating = try c.decodeIfPresent(Int.self, forKey: .rating) ?? 0
        findings = try c.decodeIfPresent([Finding].self, forKey: .findings) ?? []
    }
}

/// Extracts a run's answer from the raw (unsanitized) log tail:
///
/// 1. `finalAnswer(raw)`.
/// 2. The last JSON object with summary, rating and findings keys, a 1...10
///    rating and not the plan schema example → `.findings(rating:)`
///    (`ParsePlanOutput`).
/// 3. Else the last object with summary and findings keys, no rating key, not
///    the reviewer schema example → `.findings(rating: nil)`
///    (`ParseReviewerOutput`). An object with a rating outside 1...10 is
///    rejected, not demoted to a review.
/// 4. Else, when the answer looks like JSON (starts with "{", or a ```json
///    fence) → `.failed` with the decode reason.
/// 5. Else a non-empty sanitized answer → `.markdown`, unless the log is a
///    codex transcript with no answer header: that text is the echoed prompt
///    and tool output, not an answer.
/// 6. Else `.failed("no answer in the log")`.
public func parseRunResult(raw: String) -> RunResult {
    let answer = finalAnswer(raw)
    // A codex transcript with no answer header holds only the echoed prompt
    // and tool output. The prompt's own examples (the clean-review
    // {"summary": "No issues found.", "findings": []}) must never pass as
    // the answer, so this check comes before the JSON scan.
    if answer == raw && isCodexTranscript(raw) { return .failed("no answer in the log") }

    // Key presence is real presence, as in the rival CLI.
    var candidates: [(text: Substring, hasRating: Bool)] = []
    for obj in jsonObjects(answer) {
        guard let dict = try? JSONSerialization.jsonObject(with: Data(obj.utf8)) as? [String: Any],
              dict["summary"] != nil, dict["findings"] != nil else { continue }
        candidates.append((obj, dict["rating"] != nil))
    }

    let decoder = JSONDecoder()
    var lastError: String?
    func decode(_ text: Substring) -> Payload? {
        do {
            return try decoder.decode(Payload.self, from: Data(text.utf8))
        } catch {
            lastError = decodeReason(error)
            return nil
        }
    }
    func clean(_ f: [Finding]) -> [Finding] { sortedFindings(f.filter { !isPlaceholderFinding($0) }) }

    for c in candidates.reversed() where c.hasRating {
        guard let p = decode(c.text),
              p.summary.trimmingCharacters(in: .whitespacesAndNewlines) != planExampleSummary,
              (1...10).contains(p.rating) else { continue }
        return .findings(summary: p.summary, rating: p.rating, findings: clean(p.findings))
    }
    for c in candidates.reversed() where !c.hasRating {
        guard let p = decode(c.text),
              p.summary.trimmingCharacters(in: .whitespacesAndNewlines) != reviewerExampleSummary else { continue }
        return .findings(summary: p.summary, rating: nil, findings: clean(p.findings))
    }

    let trimmed = answer.trimmingCharacters(in: .whitespacesAndNewlines)
    if let json = jsonLooking(trimmed) {
        let reason = lastError ?? wholeDecodeError(json) ?? "no summary/findings keys"
        return .failed("JSON answer did not decode: " + reason)
    }
    let text = sanitizeLog(answer).trimmingCharacters(in: .whitespacesAndNewlines)
    if text.isEmpty { return .failed("no answer in the log") }
    return .markdown(text)
}

/// A codex log: it opens with the "OpenAI Codex" banner, or (a tail that cut
/// the banner) has an "exec" tool line.
private func isCodexTranscript(_ raw: String) -> Bool {
    if raw.drop(while: { $0.isWhitespace }).hasPrefix("OpenAI Codex") { return true }
    return raw.split(separator: "\n", omittingEmptySubsequences: false).contains { $0 == "exec" }
}

/// The JSON body of an answer that looks like JSON: it starts with "{", or
/// with a ```json fence followed by "{". nil otherwise.
private func jsonLooking(_ s: String) -> Substring? {
    if s.hasPrefix("{") { return s[...] }
    guard s.hasPrefix("```") else { return nil }
    var body = s.drop { $0 != "\n" }.dropFirst()
    if body.hasSuffix("```") { body = body.dropLast(3) }
    let t = body.drop { $0.isWhitespace }
    return t.hasPrefix("{") ? t : nil
}

/// Why `s` as a whole is not JSON, or nil when it is.
private func wholeDecodeError(_ s: Substring) -> String? {
    do {
        _ = try JSONSerialization.jsonObject(with: Data(s.utf8))
        return nil
    } catch {
        return decodeReason(error)
    }
}

/// A short reason from a Foundation JSON error: the debug description when
/// there is one, else the localized text.
private func decodeReason(_ error: Error) -> String {
    switch error {
    case DecodingError.typeMismatch(_, let ctx), DecodingError.valueNotFound(_, let ctx),
         DecodingError.keyNotFound(_, let ctx), DecodingError.dataCorrupted(let ctx):
        let path = ctx.codingPath.map { $0.intValue.map(String.init) ?? $0.stringValue }.joined(separator: ".")
        return path.isEmpty ? ctx.debugDescription : "\(path): \(ctx.debugDescription)"
    default:
        let ns = error as NSError
        return (ns.userInfo[NSDebugDescriptionErrorKey] as? String) ?? ns.localizedDescription
    }
}

// MARK: - Grouping

/// The findings of one severity bucket, as the Result tab lists them.
public struct SeverityGroup: Equatable, Sendable {
    /// "critical", "high", "medium", "low", or "other" for anything else.
    public let severity: String
    public let findings: [Finding]
}

/// `findings` bucketed by severity in ladder order, unknown severities last as
/// "other". Empty buckets are left out; order inside a bucket is kept.
public func severityGroups(_ findings: [Finding]) -> [SeverityGroup] {
    var buckets = Array(repeating: [Finding](), count: severityNames.count + 1)
    for f in findings { buckets[severityRank(f.severity)].append(f) }
    return zip(severityNames + ["other"], buckets).compactMap { name, fs in
        fs.isEmpty ? nil : SeverityGroup(severity: name, findings: fs)
    }
}
