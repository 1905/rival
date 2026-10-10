import XCTest
@testable import RivalKit

/// The `rival config … --json` bridge. Fixtures in Fixtures/config: `show`
/// output captured from the CLI, `models` and `check` lines written to the
/// Rust `to_json` field set. No process is ever started here.
final class RivalCLITests: XCTestCase {
    func configFixture(_ name: String) throws -> Data {
        try Data(contentsOf: fixturesDir.appendingPathComponent("config", isDirectory: true).appendingPathComponent(name))
    }

    // MARK: show

    func testShowParsesValuesAndSources() throws {
        let show = try parseConfigShow(configFixture("show.json"))
        XCTAssertEqual(show.configFile, "/Users/me/.rival/config.yaml")
        XCTAssertEqual(show.string("proxy.url"), "http://127.0.0.1:8317")
        XCTAssertEqual(show.source("proxy.url"), "file")
        XCTAssertTrue(show.bool("proxy.claude.enabled"))
        XCTAssertTrue(show.bool("proxy.codex.enabled"))
        XCTAssertEqual(show.string("proxy.claude.model_prefix"), "emcd2_")
        XCTAssertEqual(show.string("proxy.codex.model_prefix"), "")
        XCTAssertEqual(show.list("plan.models"), ["codex", "claude"])
        XCTAssertEqual(show.string("efforts.sol"), "xhigh")
        XCTAssertEqual(show.source("efforts.sol"), "file")
        XCTAssertEqual(show.string("efforts.kimi-k3"), "max")
        XCTAssertEqual(show.values["auto_fix_critical_high"]?.value, .bool(false))
        XCTAssertTrue(show.keySet)
        XCTAssertEqual(show.keyTail, "a91f")
    }

    func testShowDefaults() throws {
        let show = try parseConfigShow(configFixture("show-default.json"))
        XCTAssertEqual(show.string("proxy.url"), "")
        XCTAssertEqual(show.source("proxy.url"), "default")
        XCTAssertFalse(show.bool("proxy.claude.enabled"))
        XCTAssertEqual(show.list("plan.models"), ["codex"])
        XCTAssertFalse(show.keySet)
        XCTAssertNil(show.keyTail)
        // A key too short for a tail shows as plain "set".
        var short = show
        short.values["proxy.key"] = .init(value: .string("set"), source: "file", error: nil)
        XCTAssertTrue(short.keySet)
        XCTAssertNil(short.keyTail)
    }

    // MARK: models

    func testModelsParseAndPrefixes() throws {
        let models = try parseProxyModels(configFixture("models.json"))
        XCTAssertEqual(models.url, "http://127.0.0.1:8317")
        XCTAssertEqual(models.count, 6)
        XCTAssertEqual(models.models.first, "emcd_/claude-opus-5-5")
        XCTAssertEqual(models.prefixes[""], ["gpt-6-astra", "gpt-6.1-sol"])
        XCTAssertEqual(models.prefixes(serving: ModelCatalog.models(provider: "claude")), ["emcd2_", "emcd_"])
        XCTAssertEqual(models.prefixes(serving: ModelCatalog.models(provider: "codex")), [""])
    }

    // MARK: check

    func checkEvents() throws -> [CheckEvent] {
        let text = String(decoding: try configFixture("check.jsonl"), as: UTF8.self)
        return try text.split(separator: "\n", omittingEmptySubsequences: false)
            .compactMap { try parseCheckLine(String($0)) }
    }

    func testCheckRows() throws {
        let events = try checkEvents()
        XCTAssertEqual(events.count, 6)
        let rows: [CheckRow] = events.compactMap { if case .row(let r) = $0 { return r } else { return nil } }
        XCTAssertEqual(rows.map(\.name), ["codex", "sol", "claude", "fable", "k3"])
        XCTAssertEqual(rows.map(\.state), [.ok, .failed, .limit, .unexpected, .ok])

        XCTAssertEqual(rows[0].wireModel, "gpt-6-astra")
        XCTAssertEqual(rows[0].route, "proxy")
        XCTAssertEqual(rows[0].latency, "3.4s")
        XCTAssertEqual(rows[0].message, "ok")

        XCTAssertEqual(rows[1].latency, "—")
        XCTAssertEqual(rows[1].message, "proxy does not serve gpt-6.1-sol")

        XCTAssertTrue(rows[2].limit)
        XCTAssertTrue(rows[2].hint.hasPrefix("emcd_ also serves"))

        XCTAssertEqual(rows[3].message, "OK!")
        // A line without `called` (an older CLI) counts a latency as a call.
        XCTAssertTrue(rows[4].called)
        XCTAssertEqual(rows[4].runtime, "opencode")
    }

    func testCheckSummary() throws {
        guard case .summary(let s)? = try checkEvents().last else { return XCTFail("no summary line") }
        XCTAssertEqual(s.ok, 3)
        XCTAssertEqual(s.total, 5)
        XCTAssertEqual(s.text, "3 of 5 ok")
        XCTAssertFalse(s.allOK)
        XCTAssertEqual(s.proxy, CheckProxy(state: "up", url: "http://127.0.0.1:8317", models: 38, keyTail: "a91f"))
    }

    func testCheckSummaryProxyOffAndDown() throws {
        guard case .summary(let off) = try parseCheckLine(#"{"proxy":{"state":"off"},"summary":{"ok":2,"total":2}}"#)
        else { return XCTFail() }
        XCTAssertEqual(off.proxy.state, "off")
        XCTAssertTrue(off.allOK)
        guard case .summary(let down) = try parseCheckLine(
            #"{"proxy":{"state":"down","url":"http://x","error":"connection refused"},"summary":{"ok":0,"total":1}}"#)
        else { return XCTFail() }
        XCTAssertEqual(down.proxy.error, "connection refused")
    }

    func testBlankAndBadLines() {
        XCTAssertNil(try parseCheckLine("   "))
        XCTAssertThrowsError(try parseCheckLine("warning: not json"))
    }

    func testTakeLinesKeepsPartialTail() {
        var buffer = Data("{\"a\":1}\n{\"b\":".utf8)
        XCTAssertEqual(takeLines(&buffer), ["{\"a\":1}"])
        XCTAssertEqual(String(decoding: buffer, as: UTF8.self), "{\"b\":")
        buffer.append(Data("2}\n".utf8))
        XCTAssertEqual(takeLines(&buffer), ["{\"b\":2}"])
        XCTAssertTrue(buffer.isEmpty)
    }

    func testCheckNote() throws {
        let rows = try checkEvents().compactMap { e -> CheckRow? in if case .row(let r) = e { return r } else { return nil } }
        let note = checkNote(rows: rows, summary: CheckSummary(proxy: CheckProxy(state: "up"), ok: 3, total: 5))
        XCTAssertEqual(note.title, "✗ model check · 3 of 5 ok")
        XCTAssertEqual(note.body, "failed: sol, claude")
        XCTAssertEqual(note.runID, checkNoteID)
        let good = checkNote(rows: [], summary: CheckSummary(proxy: CheckProxy(state: "off"), ok: 4, total: 4))
        XCTAssertEqual(good.title, "✓ model check · 4 of 4 ok")
        XCTAssertEqual(good.body, "")
    }

    // MARK: patch

    func testPatchEncodesDottedKeysAndNull() throws {
        let patch: [String: JSONValue] = [
            "proxy.claude.enabled": .bool(true),
            "plan.models": .array([.string("codex"), .string("fable")]),
            "efforts.sol": .null,
        ]
        let data = try JSONEncoder().encode(patch)
        let back = try JSONDecoder().decode([String: JSONValue].self, from: data)
        XCTAssertEqual(back, patch)
        let text = String(decoding: data, as: UTF8.self)
        XCTAssertTrue(text.contains(#""efforts.sol":null"#), text)
    }

    // MARK: models and wire ids

    func testWireModel() {
        XCTAssertEqual(wireModel(prefix: "emcd_", model: "claude-opus-5-5"), "emcd_/claude-opus-5-5")
        XCTAssertEqual(wireModel(prefix: "emcd_/", model: "claude-opus-5-5"), "emcd_/claude-opus-5-5")
        XCTAssertEqual(wireModel(prefix: "", model: "gpt-6-astra"), "gpt-6-astra")
    }

    func testPlanNames() {
        XCTAssertEqual(ModelCatalog.planNames(["claude", "codex"]), ["codex", "claude"])
        XCTAssertEqual(ModelCatalog.planNames(["opus", "sol"]), ["sol", "claude"])
        XCTAssertEqual(ModelCatalog.all.first { $0.id == "kimi-k3" }?.efforts, ["max"])
        XCTAssertNil(ModelCatalog.all.first { $0.id == "grok" }?.provider)
    }

    // MARK: locator

    func testLocatorOrder() {
        let home = "/Users/me"
        XCTAssertEqual(
            RivalLocator.candidates(environment: ["RIVAL_BIN": "/opt/dev/rival"], home: home),
            ["/opt/dev/rival", "/opt/homebrew/bin/rival", "/usr/local/bin/rival",
             "/Users/me/.local/bin/rival", "/Users/me/.cargo/bin/rival"])
        XCTAssertEqual(RivalLocator.candidates(environment: ["RIVAL_BIN": ""], home: home).first, "/opt/homebrew/bin/rival")
    }

    func testLocatorPicksFirstExecutable() {
        var shellRan = false
        let shell = { () -> RivalLocator.LoginShell? in
            shellRan = true
            return RivalLocator.LoginShell(path: "/x", rival: "/nix/bin/rival")
        }
        let found = RivalLocator.locate(
            environment: ["RIVAL_BIN": "/missing/rival"], home: "/Users/me",
            isExecutable: { ["/usr/local/bin/rival", "/Users/me/.local/bin/rival"].contains($0) },
            loginShell: shell)
        XCTAssertEqual(found, "/usr/local/bin/rival")
        XCTAssertFalse(shellRan)

        let env = RivalLocator.locate(
            environment: ["RIVAL_BIN": "/opt/dev/rival"], home: "/Users/me",
            isExecutable: { _ in true }, loginShell: shell)
        XCTAssertEqual(env, "/opt/dev/rival")
    }

    func testLocatorFallsBackToLoginShell() {
        let found = RivalLocator.locate(
            environment: [:], home: "/Users/me",
            isExecutable: { $0 == "/nix/bin/rival" },
            loginShell: { RivalLocator.LoginShell(path: nil, rival: "/nix/bin/rival") })
        XCTAssertEqual(found, "/nix/bin/rival")
        XCTAssertNil(RivalLocator.locate(
            environment: [:], home: "/Users/me", isExecutable: { _ in false },
            loginShell: { RivalLocator.LoginShell(path: nil, rival: "/nix/bin/rival") }))
        XCTAssertNil(RivalLocator.locate(
            environment: [:], home: "/Users/me", isExecutable: { $0 == "/nix/bin/rival" }, loginShell: { nil }))
    }

    func testParseLoginShellIgnoresBanner() {
        let out = "Welcome!\nRIVAL_PATH=/opt/homebrew/bin:/usr/bin\nRIVAL_BIN=/opt/homebrew/bin/rival\n"
        XCTAssertEqual(RivalLocator.parseLoginShell(out),
                       RivalLocator.LoginShell(path: "/opt/homebrew/bin:/usr/bin", rival: "/opt/homebrew/bin/rival"))
        // `command -v` prints nothing, or an alias line, when there is no binary.
        XCTAssertEqual(RivalLocator.parseLoginShell("RIVAL_PATH=/usr/bin\nRIVAL_BIN=\n").rival, nil)
        XCTAssertEqual(RivalLocator.parseLoginShell("RIVAL_BIN=alias rival=foo\n").rival, nil)
    }

    func testChildEnvironmentUsesLoginPATH() {
        let env = RivalLocator.childEnvironment(
            base: ["PATH": "/usr/bin:/bin", "RIVAL_HOME": "/tmp/r"],
            loginPATH: "/opt/homebrew/bin:/usr/bin", executable: "/Users/me/.cargo/bin/rival")
        XCTAssertEqual(env["PATH"], "/Users/me/.cargo/bin:/opt/homebrew/bin:/usr/bin")
        XCTAssertEqual(env["RIVAL_HOME"], "/tmp/r")
        let same = RivalLocator.childEnvironment(base: ["PATH": "/usr/bin"], loginPATH: nil, executable: "/usr/bin/rival")
        XCTAssertEqual(same["PATH"], "/usr/bin")
    }

    func testLastErrorLine() {
        XCTAssertEqual(lastErrorLine(Data("note\nerror: proxy.url is not set\n\n".utf8)), "proxy.url is not set")
        XCTAssertEqual(lastErrorLine(Data()), "")
    }
}
