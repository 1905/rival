import Foundation

// The bridge to `rival config … --json`, used by the Settings window and
// the menu-bar model check. The app never writes config.yaml or the key
// file itself: every write goes through the CLI, the key through stdin.
//
// The parsing functions are pure and free of Process, so the tests feed
// them fixtures captured from the CLI.

// MARK: - JSON values

/// Any JSON value. `config show` values are strings, booleans or lists;
/// the patch for `config set --json` is built from these too.
public enum JSONValue: Codable, Equatable, Sendable {
    case string(String)
    case bool(Bool)
    case number(Double)
    case array([JSONValue])
    case object([String: JSONValue])
    case null

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? c.decode(Double.self) {
            self = .number(n)
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
        } else {
            self = .object(try c.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .string(let s): try c.encode(s)
        case .bool(let b): try c.encode(b)
        case .number(let n): try c.encode(n)
        case .array(let a): try c.encode(a)
        case .object(let o): try c.encode(o)
        case .null: try c.encodeNil()
        }
    }

    public var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    public var boolValue: Bool? {
        if case .bool(let b) = self { return b }
        return nil
    }

    /// A list of names; a comma-separated string counts as a list too.
    public var stringList: [String]? {
        switch self {
        case .array(let items): return items.compactMap(\.stringValue)
        case .string(let s):
            return s.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        default: return nil
        }
    }
}

// MARK: - config show

/// `rival config show --json`: every resolved value with its source.
public struct ConfigShow: Decodable, Equatable, Sendable {
    public struct Entry: Decodable, Equatable, Sendable {
        public var value: JSONValue
        /// `default`, `file` or `env`.
        public var source: String
        /// Set when the value is unusable (an invalid `RIVAL_PROXY_URL`).
        public var error: String?
    }

    public var configFile: String
    /// Keyed by dotted path: `proxy.url`, `proxy.claude.enabled`, `efforts.sol`…
    public var values: [String: Entry]

    enum CodingKeys: String, CodingKey {
        case configFile = "config_file"
        case values
    }

    public init(configFile: String, values: [String: Entry]) {
        self.configFile = configFile
        self.values = values
    }

    public func string(_ key: String) -> String { values[key]?.value.stringValue ?? "" }
    public func bool(_ key: String) -> Bool { values[key]?.value.boolValue ?? false }
    public func list(_ key: String) -> [String] { values[key]?.value.stringList ?? [] }
    public func source(_ key: String) -> String { values[key]?.source ?? "default" }

    /// The last characters of the stored key: `set (…a91f)` gives `a91f`.
    /// Nil when the key is missing, unreadable, or too short to show a tail.
    public var keyTail: String? {
        let shown = string("proxy.key")
        let head = "set (…"
        guard shown.hasPrefix(head), shown.hasSuffix(")") else { return nil }
        return String(shown.dropFirst(head.count).dropLast())
    }

    /// The CLI found a key (in `RIVAL_PROXY_KEY` or the key file).
    public var keySet: Bool { string("proxy.key").hasPrefix("set") }
}

public func parseConfigShow(_ data: Data) throws -> ConfigShow {
    try JSONDecoder().decode(ConfigShow.self, from: data)
}

// MARK: - config models

/// `rival config models --json`: what the proxy serves.
public struct ProxyModels: Decodable, Equatable, Sendable {
    public var url: String
    public var count: Int
    /// Wire ids in proxy order.
    public var models: [String]
    /// Bare model ids under each prefix; "" holds the ids without one.
    public var prefixes: [String: [String]]

    public init(url: String, count: Int, models: [String], prefixes: [String: [String]]) {
        self.url = url
        self.count = count
        self.models = models
        self.prefixes = prefixes
    }

    /// The prefixes that serve at least one of `models`, sorted; "" first.
    public func prefixes(serving models: [String]) -> [String] {
        prefixes.filter { _, ids in ids.contains { models.contains($0) } }
            .keys.sorted()
    }
}

public func parseProxyModels(_ data: Data) throws -> ProxyModels {
    try JSONDecoder().decode(ProxyModels.self, from: data)
}

// MARK: - config check

/// One `rival config check --json` row line.
public struct CheckRow: Decodable, Equatable, Sendable, Identifiable {
    public var name: String
    /// The session `cli`: codex, claude, opencode or grok.
    public var runtime: String
    /// `proxy` or `direct`.
    public var route: String
    public var wireModel: String
    public var ok: Bool
    /// The live call's latency; 0 when no call ran.
    public var ms: Int
    public var reply: String
    public var error: String
    public var hint: String
    /// A 429: the route works, the account is at its limit.
    public var limit: Bool
    /// A pass whose reply is not `ok`.
    public var unexpected: Bool
    /// Whether the live call ran.
    public var called: Bool

    public var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, runtime, route, ok, ms, reply, error, hint, limit, unexpected, called
        case wireModel = "wire_model"
    }

    public init(name: String, runtime: String = "", route: String = "", wireModel: String = "",
                ok: Bool, ms: Int = 0, reply: String = "", error: String = "", hint: String = "",
                limit: Bool = false, unexpected: Bool = false, called: Bool = false) {
        self.name = name
        self.runtime = runtime
        self.route = route
        self.wireModel = wireModel
        self.ok = ok
        self.ms = ms
        self.reply = reply
        self.error = error
        self.hint = hint
        self.limit = limit
        self.unexpected = unexpected
        self.called = called
    }

    // Newer fields are optional, so an older CLI's lines still parse.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        runtime = try c.decodeIfPresent(String.self, forKey: .runtime) ?? ""
        route = try c.decodeIfPresent(String.self, forKey: .route) ?? ""
        wireModel = try c.decodeIfPresent(String.self, forKey: .wireModel) ?? ""
        ok = try c.decode(Bool.self, forKey: .ok)
        let ms = try c.decodeIfPresent(Int.self, forKey: .ms) ?? 0
        self.ms = ms
        reply = try c.decodeIfPresent(String.self, forKey: .reply) ?? ""
        error = try c.decodeIfPresent(String.self, forKey: .error) ?? ""
        hint = try c.decodeIfPresent(String.self, forKey: .hint) ?? ""
        limit = try c.decodeIfPresent(Bool.self, forKey: .limit) ?? false
        unexpected = try c.decodeIfPresent(Bool.self, forKey: .unexpected) ?? false
        called = try c.decodeIfPresent(Bool.self, forKey: .called) ?? (ms > 0)
    }

    public var state: CheckState {
        if ok { return unexpected ? .unexpected : .ok }
        return limit ? .limit : .failed
    }

    /// "2.1s", or "—" when no live call ran.
    public var latency: String {
        guard called || ms > 0 else { return "—" }
        return String(format: "%.1fs", Double(ms) / 1000)
    }

    /// The reply on a pass, else the error.
    public var message: String { ok ? reply : error }
}

public enum CheckState: Sendable {
    /// Passed with the reply `ok`.
    case ok
    /// Passed, but the reply was something else (yellow).
    case unexpected
    /// A 429: the account is at its limit (orange).
    case limit
    /// Any other failure (red).
    case failed
}

/// The proxy line of the check summary.
public struct CheckProxy: Decodable, Equatable, Sendable {
    /// `off`, `up` or `down`.
    public var state: String
    public var url: String
    /// The number of models `/v1/models` listed; 0 unless `up`.
    public var models: Int
    /// The last 4 characters of the key; "" for a short key.
    public var keyTail: String
    /// Why the proxy is `down`.
    public var error: String

    enum CodingKeys: String, CodingKey {
        case state, url, models, error
        case keyTail = "key_tail"
    }

    public init(state: String, url: String = "", models: Int = 0, keyTail: String = "", error: String = "") {
        self.state = state
        self.url = url
        self.models = models
        self.keyTail = keyTail
        self.error = error
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = try c.decode(String.self, forKey: .state)
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        models = try c.decodeIfPresent(Int.self, forKey: .models) ?? 0
        keyTail = try c.decodeIfPresent(String.self, forKey: .keyTail) ?? ""
        error = try c.decodeIfPresent(String.self, forKey: .error) ?? ""
    }
}

/// The last `config check --json` line: the proxy and `N of M ok`.
public struct CheckSummary: Decodable, Equatable, Sendable {
    public var proxy: CheckProxy
    public var ok: Int
    public var total: Int

    struct Counts: Decodable {
        var ok: Int
        var total: Int
    }

    enum CodingKeys: String, CodingKey { case proxy, summary }

    public init(proxy: CheckProxy, ok: Int, total: Int) {
        self.proxy = proxy
        self.ok = ok
        self.total = total
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let counts = try c.decode(Counts.self, forKey: .summary)
        proxy = try c.decodeIfPresent(CheckProxy.self, forKey: .proxy) ?? CheckProxy(state: "off")
        ok = counts.ok
        total = counts.total
    }

    public var allOK: Bool { ok == total }
    /// "4 of 5 ok".
    public var text: String { "\(ok) of \(total) ok" }
}

/// One `config check --json` line.
public enum CheckEvent: Equatable, Sendable {
    case row(CheckRow)
    case summary(CheckSummary)
}

/// Parses one `config check --json` line. Nil for a blank line. The summary
/// line is the one with a `summary` key.
public func parseCheckLine(_ line: String) throws -> CheckEvent? {
    let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return nil }
    let data = Data(trimmed.utf8)
    struct Probe: Decodable { var summary: JSONValue? }
    if try JSONDecoder().decode(Probe.self, from: data).summary != nil {
        return .summary(try JSONDecoder().decode(CheckSummary.self, from: data))
    }
    return .row(try JSONDecoder().decode(CheckRow.self, from: data))
}

/// Removes every complete line from the front of `buffer` and returns them.
/// A partial last line stays in `buffer` for the next read.
public func takeLines(_ buffer: inout Data) -> [String] {
    var lines: [String] = []
    while let nl = buffer.firstIndex(of: 0x0A) {
        let line = buffer[buffer.startIndex..<nl]
        lines.append(String(decoding: line, as: UTF8.self))
        buffer.removeSubrange(buffer.startIndex...nl)
    }
    return lines
}

/// The menu-bar check notification: "✓ model check · 5 of 5 ok", or
/// "✗ model check · 4 of 5 ok" with the failed names in the body.
public func checkNote(rows: [CheckRow], summary: CheckSummary) -> FinishNote {
    let failed = rows.filter { !$0.ok }.map(\.name)
    let title = (summary.allOK ? "✓ " : "✗ ") + "model check · " + summary.text
    let body = failed.isEmpty ? "" : "failed: " + failed.joined(separator: ", ")
    return FinishNote(runID: checkNoteID, title: title, body: body)
}

/// A check that could not run at all.
public func checkFailedNote(_ message: String) -> FinishNote {
    FinishNote(runID: checkNoteID, title: "✗ model check failed", body: String(message.prefix(200)))
}

/// The notification id of the model check; a click opens Settings, not a run.
public let checkNoteID = "check:models"

// MARK: - Models the settings show

/// One reviewer model in the Models tab and the "Requests go to" list.
public struct ModelEntry: Identifiable, Equatable, Sendable {
    /// The `efforts.<id>` label.
    public let id: String
    public let title: String
    /// The session `cli`.
    public let runtime: String
    /// The bare model id.
    public let model: String
    /// The `plan.models` name; nil for models plan cannot use.
    public let planName: String?

    /// `claude` or `codex` for the runtimes the proxy carries.
    public var provider: String? { runtime == "claude" || runtime == "codex" ? runtime : nil }

    /// The `efforts.<id>` values the CLI accepts. K3 is pinned to max.
    public var efforts: [String] {
        id == "kimi-k3" ? ["max"] : ["low", "medium", "high", "xhigh", "ultra"]
    }
}

public enum ModelCatalog {
    /// In the CLI's check order.
    public static let all: [ModelEntry] = [
        ModelEntry(id: "codex", title: "Codex", runtime: "codex", model: "gpt-6-astra", planName: "codex"),
        ModelEntry(id: "sol", title: "Sol 6.1", runtime: "codex", model: "gpt-6.1-sol", planName: "sol"),
        ModelEntry(id: "claude", title: "Opus 5.5", runtime: "claude", model: "claude-opus-5-5", planName: "claude"),
        ModelEntry(id: "fable", title: "Fable 5.1", runtime: "claude", model: "claude-fable-5-1", planName: "fable"),
        ModelEntry(id: "kimi-k3", title: "Kimi K3", runtime: "opencode", model: "moonshotai/kimi-k3", planName: nil),
        ModelEntry(id: "grok", title: "Grok 4.6", runtime: "grok", model: "grok-4.6", planName: nil),
    ]

    /// The models a proxy provider carries.
    public static func models(provider: String) -> [String] {
        all.filter { $0.provider == provider }.map(\.model)
    }

    /// The plan names in catalog order; `opus` reads as `claude`.
    public static func planNames(_ names: [String]) -> [String] {
        let set = Set(names.map { $0.lowercased() == "opus" ? "claude" : $0.lowercased() })
        return all.compactMap(\.planName).filter { set.contains($0) }
    }
}

/// The model id on the wire: `<prefix>/<model>`, or the bare model with no
/// prefix. A trailing `/` on the prefix is dropped, as the CLI does.
public func wireModel(prefix: String, model: String) -> String {
    var p = prefix.trimmingCharacters(in: .whitespaces)
    while p.hasSuffix("/") { p.removeLast() }
    return p.isEmpty ? model : p + "/" + model
}

// MARK: - Locating the CLI

public enum RivalLocator {
    /// Shown when no `rival` binary is found.
    public static let installCommand = "brew install 1905/tap/rival"

    /// The fixed places, in order: `RIVAL_BIN`, Homebrew (Apple silicon,
    /// Intel), `~/.local/bin`, `~/.cargo/bin`. The login shell comes after.
    public static func candidates(environment: [String: String], home: String) -> [String] {
        var out: [String] = []
        if let bin = environment["RIVAL_BIN"], !bin.isEmpty {
            out.append((bin as NSString).expandingTildeInPath)
        }
        out += [
            "/opt/homebrew/bin/rival",
            "/usr/local/bin/rival",
            home + "/.local/bin/rival",
            home + "/.cargo/bin/rival",
        ]
        return out
    }

    /// The first executable candidate, else the login shell's `rival`.
    /// `loginShell` runs only when no fixed place has one.
    public static func locate(
        environment: [String: String],
        home: String,
        isExecutable: (String) -> Bool,
        loginShell: () -> LoginShell?
    ) -> String? {
        if let hit = candidates(environment: environment, home: home).first(where: isExecutable) {
            return hit
        }
        guard let bin = loginShell()?.rival, !bin.isEmpty, isExecutable(bin) else { return nil }
        return bin
    }

    /// What the login shell knows: its `PATH` and `command -v rival`.
    public struct LoginShell: Equatable, Sendable {
        public var path: String?
        public var rival: String?

        public init(path: String?, rival: String?) {
            self.path = path
            self.rival = rival
        }
    }

    /// The marked lines of `loginShellScript`. Anything else (a profile that
    /// prints a banner) is ignored.
    public static func parseLoginShell(_ output: String) -> LoginShell {
        var shell = LoginShell(path: nil, rival: nil)
        for line in output.split(separator: "\n") {
            if line.hasPrefix(pathMark) {
                let v = String(line.dropFirst(pathMark.count))
                shell.path = v.isEmpty ? nil : v
            } else if line.hasPrefix(binMark) {
                let v = String(line.dropFirst(binMark.count))
                shell.rival = v.hasPrefix("/") ? v : nil
            }
        }
        return shell
    }

    static let pathMark = "RIVAL_PATH="
    static let binMark = "RIVAL_BIN="
    static let loginShellScript =
        "printf '\(pathMark)%s\\n' \"$PATH\"; printf '\(binMark)%s\\n' \"$(command -v rival)\""

    /// `/bin/zsh -lc …`, killed after 5 s (a profile that waits for input).
    /// Blocking: call it off the main thread.
    public static func runLoginShell() -> LoginShell? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/zsh")
        process.arguments = ["-lc", loginShellScript]
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        process.standardInput = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let deadline = Date().addingTimeInterval(5)
        while process.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        if process.isRunning {
            process.terminate()
            return nil
        }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        return parseLoginShell(String(decoding: data, as: UTF8.self))
    }

    /// The child environment: the app's own, with the login shell's `PATH`
    /// (a Finder-launched app has only the system one, and the check looks
    /// for `claude` and `codex` on it) and the binary's own directory first.
    public static func childEnvironment(base: [String: String], loginPATH: String?, executable: String) -> [String: String] {
        var env = base
        var parts = (loginPATH ?? base["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin")
            .split(separator: ":").map(String.init)
        let dir = (executable as NSString).deletingLastPathComponent
        if !dir.isEmpty, !parts.contains(dir) { parts.insert(dir, at: 0) }
        env["PATH"] = parts.joined(separator: ":")
        return env
    }
}

// MARK: - Running the CLI

public enum RivalCLIError: Error, LocalizedError, Equatable {
    case notFound
    /// The command exited non-zero; `message` is its last stderr line.
    case failed(command: String, status: Int32, message: String)
    case badOutput(command: String, detail: String)

    public var errorDescription: String? {
        switch self {
        case .notFound:
            return "rival CLI not found"
        case .failed(let command, let status, let message):
            return message.isEmpty ? "rival \(command) exited \(status)" : message
        case .badOutput(let command, let detail):
            return "rival \(command): unexpected output (\(detail))"
        }
    }
}

/// The last non-empty line of a command's stderr, without the `error: `
/// prefix the CLI prints.
public func lastErrorLine(_ data: Data) -> String {
    let text = String(decoding: data, as: UTF8.self)
    let line = text.split(separator: "\n").lazy
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .last { !$0.isEmpty } ?? ""
    return line.hasPrefix("error: ") ? String(line.dropFirst("error: ".count)) : line
}

/// A located `rival` binary and the environment it runs with.
public struct RivalCLI: Sendable {
    public let executable: String
    public let environment: [String: String]

    public init(executable: String, environment: [String: String]) {
        self.executable = executable
        self.environment = environment
    }

    /// Finds `rival` (see `RivalLocator`). Runs the login shell once, for
    /// its `PATH`. Blocking: call it off the main thread.
    public static func locate(environment: [String: String] = ProcessInfo.processInfo.environment) -> RivalCLI? {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let shell = RivalLocator.runLoginShell()
        let found = RivalLocator.locate(
            environment: environment,
            home: home,
            isExecutable: { FileManager.default.isExecutableFile(atPath: $0) },
            loginShell: { shell }
        )
        guard let exe = found else { return nil }
        let env = RivalLocator.childEnvironment(base: environment, loginPATH: shell?.path, executable: exe)
        return RivalCLI(executable: exe, environment: env)
    }

    public func show() async throws -> ConfigShow {
        let out = try await run(["config", "show", "--json"])
        do { return try parseConfigShow(out.stdout) } catch {
            throw RivalCLIError.badOutput(command: "config show", detail: "\(error)")
        }
    }

    public func models() async throws -> ProxyModels {
        let out = try await run(["config", "models", "--json"])
        do { return try parseProxyModels(out.stdout) } catch {
            throw RivalCLIError.badOutput(command: "config models", detail: "\(error)")
        }
    }

    /// Applies a patch in one write. Keys may be dotted (`proxy.claude.enabled`);
    /// `.null` removes a key. Returns the keys the CLI saved.
    @discardableResult
    public func set(patch: [String: JSONValue]) async throws -> [String] {
        let body = try JSONEncoder().encode(patch)
        let out = try await run(["config", "set", "--json"], stdin: body)
        struct Saved: Decodable { var keys: [String]? }
        return (try? JSONDecoder().decode(Saved.self, from: out.stdout).keys) ?? []
    }

    /// Stores the proxy key. The key goes through stdin, never argv.
    public func setKey(_ key: String) async throws {
        try await run(["config", "key", "set"], stdin: Data(key.utf8))
    }

    public func clearKey() async throws {
        try await run(["config", "key", "clear"])
    }

    /// `config check --json`, one event per line as each model finishes.
    /// Exit 1 (some model failed) still ends the stream normally. Ending the
    /// iteration early terminates the process.
    public func check(models: [String] = []) -> AsyncThrowingStream<CheckEvent, Error> {
        var args = ["config", "check", "--json"]
        if !models.isEmpty { args += ["-m", models.joined(separator: ",")] }
        let job = CheckJob(executable: executable, arguments: args, environment: environment)
        return AsyncThrowingStream { continuation in
            continuation.onTermination = { _ in job.cancel() }
            DispatchQueue.global(qos: .userInitiated).async { job.run(continuation) }
        }
    }

    struct Output: Sendable {
        var status: Int32
        var stdout: Data
        var stderr: Data
    }

    @discardableResult
    func run(_ args: [String], stdin: Data? = nil) async throws -> Output {
        let exe = executable, env = environment
        let out: Output = try await withCheckedThrowingContinuation { cont in
            DispatchQueue.global(qos: .userInitiated).async {
                do { cont.resume(returning: try Self.runBlocking(exe, args, env, stdin)) } catch { cont.resume(throwing: error) }
            }
        }
        guard out.status == 0 else {
            let message = lastErrorLine(out.stderr)
            throw RivalCLIError.failed(command: args.prefix(2).joined(separator: " "), status: out.status, message: message)
        }
        return out
    }

    /// The most stdin a call may carry: it is written into the pipe before
    /// launch, so it must fit the pipe buffer (64 KB).
    static let stdinLimit = 32 * 1024

    static func runBlocking(_ exe: String, _ args: [String], _ env: [String: String], _ stdin: Data?) throws -> Output {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: exe)
        process.arguments = args
        process.environment = env
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        if let stdin {
            guard stdin.count <= stdinLimit else {
                throw RivalCLIError.badOutput(command: args.joined(separator: " "), detail: "input too large")
            }
            // Written and closed before launch: the child reads it and EOF,
            // and a child that exits early cannot raise SIGPIPE here.
            let pipe = Pipe()
            pipe.fileHandleForWriting.write(stdin)
            try? pipe.fileHandleForWriting.close()
            process.standardInput = pipe
        } else {
            process.standardInput = FileHandle.nullDevice
        }
        try process.run()
        let drain = Drain(err.fileHandleForReading)
        let stdout = out.fileHandleForReading.readDataToEndOfFile()
        drain.wait()
        process.waitUntilExit()
        return Output(status: process.terminationStatus, stdout: stdout, stderr: drain.data)
    }
}

/// Reads a handle to EOF on another thread, so a full stderr pipe never
/// blocks the child while stdout is read.
final class Drain: @unchecked Sendable {
    private let group = DispatchGroup()
    private(set) var data = Data()

    init(_ handle: FileHandle) {
        group.enter()
        DispatchQueue.global(qos: .utility).async { [self] in
            data = handle.readDataToEndOfFile()
            group.leave()
        }
    }

    func wait() { group.wait() }
}

/// One `config check --json` process feeding a stream. `cancel` may come
/// from any thread, before or after launch.
final class CheckJob: @unchecked Sendable {
    private let process = Process()
    private let out = Pipe()
    private let err = Pipe()
    private let lock = NSLock()
    private var launched = false
    private var cancelled = false

    init(executable: String, arguments: [String], environment: [String: String]) {
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.environment = environment
        process.standardOutput = out
        process.standardError = err
        process.standardInput = FileHandle.nullDevice
    }

    func cancel() {
        lock.lock()
        defer { lock.unlock() }
        cancelled = true
        // `terminate` before launch raises an Objective-C exception.
        if launched, process.isRunning { process.terminate() }
    }

    func run(_ continuation: AsyncThrowingStream<CheckEvent, Error>.Continuation) {
        lock.lock()
        if cancelled {
            lock.unlock()
            continuation.finish()
            return
        }
        do {
            try process.run()
            launched = true
        } catch {
            lock.unlock()
            continuation.finish(throwing: error)
            return
        }
        lock.unlock()

        let drain = Drain(err.fileHandleForReading)
        let handle = out.fileHandleForReading
        var buffer = Data()
        var sawSummary = false
        func emit(_ line: String) {
            // A line that is not JSON (a warning) is skipped, not fatal.
            guard let event = try? parseCheckLine(line) else { return }
            if case .summary = event { sawSummary = true }
            continuation.yield(event)
        }
        while true {
            let chunk = handle.availableData
            if chunk.isEmpty { break }
            buffer.append(chunk)
            takeLines(&buffer).forEach(emit)
        }
        emit(String(decoding: buffer, as: UTF8.self))
        drain.wait()
        process.waitUntilExit()

        lock.lock()
        let wasCancelled = cancelled
        lock.unlock()
        let status = process.terminationStatus
        if wasCancelled || status == 0 || (status == 1 && sawSummary) {
            continuation.finish()
        } else {
            let message = lastErrorLine(drain.data)
            continuation.finish(throwing: RivalCLIError.failed(command: "config check", status: status, message: message))
        }
    }
}
