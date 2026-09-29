import XCTest
@testable import RivalKit

final class ResultParserTests: XCTestCase {
    // MARK: - Helpers

    private func findings(_ r: RunResult, file: StaticString = #filePath, line: UInt = #line)
        -> (summary: String, rating: Int?, findings: [Finding])? {
        guard case let .findings(s, rt, f) = r else {
            XCTFail("want .findings, got \(r)", file: file, line: line)
            return nil
        }
        return (s, rt, f)
    }

    private func failed(_ r: RunResult, file: StaticString = #filePath, line: UInt = #line) -> String? {
        guard case let .failed(msg) = r else {
            XCTFail("want .failed, got \(r)", file: file, line: line)
            return nil
        }
        return msg
    }

    /// A codex transcript with an echoed prompt that carries the schema example.
    private let codexHeader = """
    OpenAI Codex v0.155.1
    --------
    workdir: /Users/dev/src/acme-api
    model: gpt-6-astra
    --------
    user
    Plan document to review: /tmp/plan.md

    Output schema:
    ```json
    {
      "summary": "1-3 sentence overall assessment of the plan",
      "rating": 7,
      "findings": [
        {
          "file": "section or heading the issue is in (or the filename)",
          "line": 0,
          "severity": "critical|high|medium|low",
          "category": "bug|gap|ambiguity|scope|verification",
          "title": "one-line description of the issue",
          "confidence": 8
        }
      ]
    }
    ```

    exec
    /bin/zsh -lc "cat internal/billing/ledger.go" in /Users/dev/src/acme-api
    func Post(e Entry) error {
        if e.Amount == 0 {
    """

    // MARK: - Codex transcripts

    func testCodexPlanDoubleAnswerJSON() throws {
        let answer = #"{"summary":"Sound plan, one gap.","rating":6,"findings":[{"file":"Rollout","line":12,"severity":"high","category":"gap","title":"no rollback","body":"b","confidence":9}]}"#
        let raw = codexHeader + "\ncodex\n" + answer + "\nhook: Stop\nhook: Stop Completed\ntokens used\n117.735\n" + answer + "\n"
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertEqual(r.summary, "Sound plan, one gap.")
        XCTAssertEqual(r.rating, 6)
        XCTAssertEqual(r.findings.map(\.title), ["no rollback"], "decoded once, schema example skipped")
    }

    func testCodexMarkdownDoubleAnswerHasNoHookLines() {
        let md = "## Review\n\n- `ledger.go:14` drops zero entries.\n- Tests pass."
        let streamed = "## Review\nhook: PreToolUse\nhook: PreToolUse Completed\n\n- `ledger.go:14` drops zero entries.\nhook: Stop\n- Tests pass."
        let raw = codexHeader + "\ncodex\n" + streamed + "\ntokens used\n78,402\n" + md + "\n"
        XCTAssertEqual(finalAnswer(raw).trimmingCharacters(in: .whitespacesAndNewlines), md)
        XCTAssertEqual(parseRunResult(raw: raw), .markdown(md))
    }

    func testCodexFooterInsideSecondCopy() {
        let md = "Two issues.\n\n- one\n- two\n- three"
        let raw = codexHeader + "\ncodex\n" + md + "\nhook: Stop\nhook: Stop Completed\nTwo issues.\n\n- one\ntokens used\n69.540\n- two\n- three\n"
        XCTAssertEqual(parseRunResult(raw: raw), .markdown(md))
    }

    func testCodexFooterLastUsesStreamedAnswerWithoutHooks() {
        let raw = codexHeader + "\ncodex\nAll good.\nhook: Stop\nhook: Stop Completed\ntokens used\n99.836\n"
        XCTAssertEqual(parseRunResult(raw: raw), .markdown("All good."))
    }

    func testCodexWithoutFooterUsesTextAfterLastHeader() {
        XCTAssertEqual(finalAnswer("codex\nfirst\ncodex\nsecond").trimmingCharacters(in: .whitespacesAndNewlines), "second")
        XCTAssertEqual(finalAnswer("plain log"), "plain log", "no header: the whole text")
    }

    func testToolOutputJSONBeforeAnswerIsIgnored() {
        let raw = "user\nprompt…\nexec cat saved-review.json\n" +
            #"{"summary": "Saved: nothing wrong.", "findings": []}"# +
            "\ncodex\nI could not finish the review.\ntokens used\n1234\n"
        XCTAssertEqual(parseRunResult(raw: raw), .markdown("I could not finish the review."))
    }

    func testPromptEchoOnlyIsNoAnswer() {
        XCTAssertEqual(failed(parseRunResult(raw: codexHeader)), "no answer in the log")
        // A tail that cut the banner still has exec lines.
        XCTAssertEqual(failed(parseRunResult(raw: "exec\nls -la\ntotal 0\n")), "no answer in the log")
    }

    func testEmptyLogIsNoAnswer() {
        XCTAssertEqual(failed(parseRunResult(raw: "")), "no answer in the log")
        XCTAssertEqual(failed(parseRunResult(raw: "\n\u{1B}[0m  \n")), "no answer in the log")
        XCTAssertEqual(failed(parseRunResult(raw: codexHeader + "\ncodex\n\ntokens used\n12\n")), "no answer in the log")
    }

    // MARK: - Plain logs

    func testPlainJSONLog() {
        let raw = #"{"summary":"Lean.","rating":8,"findings":[{"file":"cmd/run.go","line":3,"severity":"low","category":"slop","title":"t","body":"b","confidence":6}]}"#
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertEqual(r.rating, 8)
        XCTAssertEqual(r.findings.first?.file, "cmd/run.go")
    }

    func testFencedJSONLog() {
        let raw = "```json\n{\"summary\":\"Fenced.\",\"findings\":[]}\n```\n"
        XCTAssertEqual(parseRunResult(raw: raw), .findings(summary: "Fenced.", rating: nil, findings: []))
    }

    func testReviewPayloadWithoutRating() {
        let raw = #"{"summary":"One bug.","findings":[{"file":"a.go","line":1,"severity":"high","failure_scenario":"x=0","suggestion":"guard","confidence":8}]}"#
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertNil(r.rating)
        XCTAssertEqual(r.findings, [Finding(file: "a.go", line: 1, severity: "high", failureScenario: "x=0",
                                            suggestion: "guard", confidence: 8)])
    }

    func testMarkdownAnswer() {
        let md = "# Verdict\n\nShip it.\n\n1. one\n2. two"
        XCTAssertEqual(parseRunResult(raw: md + "\n"), .markdown(md))
    }

    func testMarkdownIsSanitized() {
        XCTAssertEqual(parseRunResult(raw: "\u{1B}[1mbold\u{1B}[0m\tx"), .markdown("bold    x"))
    }

    func testBrokenJSONAnswerFails() throws {
        let msg = try XCTUnwrap(failed(parseRunResult(raw: #"{"summary":"cut off","findings":[{"file":"a.go""#)))
        XCTAssertTrue(msg.hasPrefix("JSON answer did not decode: "), msg)
        XCTAssertGreaterThan(msg.count, "JSON answer did not decode: ".count)
    }

    func testWrongTypeReportsDecodeError() throws {
        let msg = try XCTUnwrap(failed(parseRunResult(raw: #"{"summary":"s","findings":[{"file":"a.go","line":"42"}]}"#)))
        XCTAssertTrue(msg.hasPrefix("JSON answer did not decode: "), msg)
        XCTAssertTrue(msg.contains("line"), msg)
    }

    func testJSONWithoutPayloadKeysFails() {
        XCTAssertEqual(failed(parseRunResult(raw: #"{"event":"done","ok":true}"#)),
                       "JSON answer did not decode: no summary/findings keys")
    }

    func testRatingOutOfRangeIsRejected() {
        for rating in [0, 11] {
            let raw = "{\"summary\":\"s\",\"rating\":\(rating),\"findings\":[]}"
            XCTAssertNotNil(failed(parseRunResult(raw: raw)), "rating \(rating)")
        }
    }

    func testPlanPayloadWinsOverLaterReview() {
        let raw = #"{"summary":"plan","rating":5,"findings":[]} {"summary":"review","findings":[]}"#
        XCTAssertEqual(parseRunResult(raw: raw), .findings(summary: "plan", rating: 5, findings: []))
    }

    func testPlaceholdersDroppedAndSeverityOrder() {
        let raw = #"{"summary":"s","findings":["# +
            #"{"file":"f1","severity":"low","confidence":9},"# +
            #"{"file":"path/to/file","severity":"high","confidence":9},"# +
            #"{"file":"f2","severity":"weird","confidence":9},"# +
            #"{"file":"f3","severity":"High","confidence":5},"# +
            #"{"file":"f4","severity":"critical","confidence":1},"# +
            #"{"file":"f5","severity":"medium","confidence":7},"# +
            #"{"file":"f6","severity":"high","confidence":8},"# +
            #"{"file":"f7","severity":"high","category":"bug|gap|ambiguity|scope|verification","confidence":9}"# +
            "]}"
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertEqual(r.findings.map(\.file), ["f4", "f6", "f3", "f5", "f1", "f2"])
    }

    func testSeverityRankAndStableSort() {
        XCTAssertEqual(["critical", "HIGH", "Medium", "low", "info", ""].map(severityRank), [0, 1, 2, 3, 4, 4])
        let fs = [Finding(file: "a", severity: "low", confidence: 5), Finding(file: "b", severity: "low", confidence: 5),
                  Finding(file: "c", severity: "low", confidence: 6)]
        XCTAssertEqual(sortedFindings(fs).map(\.file), ["c", "a", "b"])
    }

    func testMissingAndNullFieldsDefault() {
        let raw = #"{"summary":null,"findings":[{"file":"a.go","line":null,"suggestion":""}]}"#
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertEqual(r.summary, "")
        XCTAssertEqual(r.findings, [Finding(file: "a.go")])
    }

    // MARK: - Go parity (rival/internal/review/parse_test.go, plan_test.go)

    func testGoReviewerIgnoresEchoedSchemaExample() {
        let raw = """
        OpenAI Codex
        user
        Review scope: rival/

        ```json
        {
          "summary": "1-3 sentence reviewer summary",
          "findings": [
            {"file": "path/to/file", "line": 42, "severity": "critical|high|medium|low", "title": "brief title", "confidence": 8}
          ]
        }
        ```

        exec /bin/zsh -lc "nl -ba main.go"
        codex
        {"summary":"Found a real issue.","findings":[{"file":"rival/main.go","line":10,"severity":"high","category":"bug","title":"real bug","body":"real explanation","confidence":9}]}
        tokens used 1234
        """
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertEqual(r.summary, "Found a real issue.")
        XCTAssertEqual(r.findings.map(\.file), ["rival/main.go"])
    }

    func testGoBareObjectWithProse() {
        XCTAssertEqual(parseRunResult(raw: #"prefix noise {"summary":"ok","findings":[]} trailing"#),
                       .findings(summary: "ok", rating: nil, findings: []))
    }

    func testGoNoPayload() {
        XCTAssertEqual(parseRunResult(raw: "no json here at all"), .markdown("no json here at all"))
    }

    func testGoRejectsOnlySchemaExample() {
        let reviewer = "```json\n" + #"{"summary":"1-3 sentence reviewer summary","findings":[{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8}]}"# + "\n```"
        XCTAssertNotNil(failed(parseRunResult(raw: reviewer)))
        let plan = "```json\n" + #"{"summary":"1-3 sentence overall assessment of the plan","rating":7,"findings":[{"file":"section or heading the issue is in (or the filename)","line":0,"severity":"critical|high|medium|low","category":"bug|gap|ambiguity|scope|verification","title":"one-line description of the issue","confidence":8}]}"# + "\n```"
        XCTAssertNotNil(failed(parseRunResult(raw: plan)))
    }

    func testGoAcceptsCleanReviewAndPlan() {
        XCTAssertEqual(parseRunResult(raw: #"prose {"summary": "No issues found.", "findings": []} prose"#),
                       .findings(summary: "No issues found.", rating: nil, findings: []))
        XCTAssertEqual(parseRunResult(raw: #"prose {"summary":"Airtight.","rating":9,"findings":[]} prose"#),
                       .findings(summary: "Airtight.", rating: 9, findings: []))
    }

    func testGoDropsOnlyPlaceholderFindings() {
        let raw = #"{"summary":"real","findings":["# +
            #"{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8},"# +
            #"{"file":"real.go","line":7,"severity":"high","category":"bug","title":"real","body":"b","confidence":9}]}"#
        XCTAssertEqual(findings(parseRunResult(raw: raw))?.findings.map(\.file), ["real.go"])
    }

    func testGoNestedInsideInvalidRegion() {
        let raw = #"wrapper { not valid json but balanced: {"summary":"nested real","findings":[{"file":"a.go","line":1,"severity":"high","confidence":8}]} }"#
        XCTAssertEqual(findings(parseRunResult(raw: raw))?.summary, "nested real")
    }

    func testGoAcceptsRealFindingDiscussingEnum() {
        let raw = #"{"summary":"real review","findings":[{"file":"rival/internal/review/parse.go","line":84,"severity":"high","category":"bug","title":"placeholder check","body":"isPlaceholderFinding compares against critical|high|medium|low which is fine","confidence":9}]}"#
        XCTAssertEqual(findings(parseRunResult(raw: raw))?.findings.map(\.severity), ["high"])
    }

    func testGoUnbalancedBraceBeforeAnswer() {
        let raw = """
        {"summary":"schema","findings":[{"file":"path/to/file","line":42}]}
        exec: showing code
        func New() {           // <- unbalanced brace, never closes as JSON
            x := "a } in a string"
        codex
        {"summary":"real","findings":[{"file":"real.go","line":1,"confidence":9}]}
        """
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertEqual(r.summary, "real")
        XCTAssertEqual(r.findings.map(\.file), ["real.go"])
    }

    func testGoKeepsDualCategoryAndDropsAntislopEcho() {
        let dual = #"{"summary":"real","rating":6,"findings":[{"file":"a.go","line":3,"severity":"high","category":"bug|security","title":"real dual-category finding","body":"b","confidence":8}]}"#
        XCTAssertEqual(findings(parseRunResult(raw: dual))?.findings.map(\.title), ["real dual-category finding"])
        let echo = #"{"summary":"lean enough","rating":9,"findings":["# +
            #"{"file":"cmd/command_antislop.go","line":10,"severity":"high","category":"reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni","title":"echoed example","body":"copied","confidence":8},"# +
            #"{"file":"cmd/command_antislop.go","line":20,"severity":"medium","category":"slop","title":"real finding","body":"a real cut","confidence":7}]}"#
        XCTAssertEqual(findings(parseRunResult(raw: echo))?.findings.map(\.title), ["real finding"])
    }

    func testGoPlanRejectsUnrelatedJSON() {
        XCTAssertNotNil(failed(parseRunResult(raw: #"{"event":"done","ok":true}"#)))
    }

    func testGoRealCapturedLog() throws {
        let raw = try String(contentsOf: fixturesDir.appendingPathComponent("logs/consilium_echoed_schema.log"), encoding: .utf8)
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertFalse(r.findings.isEmpty)
        XCTAssertFalse(r.findings.contains { $0.file == "path/to/file" })
        XCTAssertNotEqual(r.summary, "1-3 sentence reviewer summary")
    }

    // MARK: - Fake fixture logs (dev_bundle.py --fixture)

    private func fakeLog(_ name: String) throws -> String {
        try String(contentsOf: fixturesDir.appendingPathComponent("logs/" + name), encoding: .utf8)
    }

    func testFakePlanCodexLog() throws {
        let raw = try fakeLog("plan-codex.log")
        XCTAssertTrue(raw.contains("path/to/file"), "the echoed schema placeholder is in the log")
        guard let r = findings(parseRunResult(raw: raw)) else { return }
        XCTAssertEqual(r.rating, 6)
        XCTAssertEqual(r.findings.count, 6)
        XCTAssertFalse(r.findings.contains { isPlaceholderFinding($0) })
        XCTAssertEqual(r.findings.map(\.severity), ["critical", "high", "high", "medium", "medium", "low"])
    }

    func testFakeReviewMarkdownLog() throws {
        guard case let .markdown(md) = parseRunResult(raw: try fakeLog("review-markdown.log")) else {
            return XCTFail("want .markdown")
        }
        XCTAssertTrue(md.hasPrefix("## Review"), md)
    }

    func testFakeBrokenJSONLog() throws {
        let msg = try XCTUnwrap(failed(parseRunResult(raw: try fakeLog("broken-json.log"))))
        XCTAssertTrue(msg.hasPrefix("JSON answer did not decode: "), msg)
    }

    // MARK: - jsonObjects

    func testJSONObjectsOrderAndNesting() {
        let objs = jsonObjects(#"a {"x":{"y":1}} b { "z": "}{" } { broken"#).map(String.init)
        XCTAssertEqual(objs, [#"{"y":1}"#, #"{"x":{"y":1}}"#, #"{ "z": "}{" }"#])
        XCTAssertEqual(jsonObjects("no braces"), [])
        XCTAssertEqual(jsonObjects(#"héllo {"k":"ü"} ✓"#).map(String.init), [#"{"k":"ü"}"#])
    }

    // MARK: - Grouping and markdown blocks

    func testSeverityGroups() {
        let fs = [Finding(file: "a", severity: "high"), Finding(file: "b", severity: "weird"),
                  Finding(file: "c", severity: "HIGH"), Finding(file: "d", severity: "low")]
        let groups = severityGroups(fs)
        XCTAssertEqual(groups.map(\.severity), ["high", "low", "other"])
        XCTAssertEqual(groups.first?.findings.map(\.file), ["a", "c"])
        XCTAssertEqual(severityGroups([]), [])
    }

    func testMarkdownBlocks() {
        let md = """
        # Title
        Intro line one
        intro line two

        - dash item
          * nested star
        12. numbered
        3) paren

        **bold** start is a paragraph
        #nospace is a paragraph
        ```swift
        let x = 1

        ```
        ```
        unclosed
        """
        XCTAssertEqual(markdownBlocks(md), [
            .heading(level: 1, text: "Title"),
            .paragraph("Intro line one\nintro line two"),
            .item(marker: "•", text: "dash item", indent: 0),
            .item(marker: "•", text: "nested star", indent: 1),
            .item(marker: "12.", text: "numbered", indent: 0),
            .item(marker: "3.", text: "paren", indent: 0),
            .paragraph("**bold** start is a paragraph\n#nospace is a paragraph"),
            .code("let x = 1\n"),
            .code("unclosed"),
        ])
        XCTAssertEqual(markdownBlocks(""), [])
    }

    func testMarkdownWrappedListItemContinues() {
        let md = "- a.go:1 - first line\n  wraps here.\nstill the item\n\nnew paragraph"
        XCTAssertEqual(markdownBlocks(md), [
            .item(marker: "•", text: "a.go:1 - first line wraps here. still the item", indent: 0),
            .paragraph("new paragraph"),
        ])
    }

    func testUnansweredCodexTranscriptIgnoresPromptCleanExample() {
        let raw = """
        OpenAI Codex v0.153.4
        --------
        workdir: /Users/dev/src/acme-api
        --------
        user
        Review the code. If the code is solid, return: {"summary": "No issues found.", "findings": []}
        exec
        /bin/zsh -lc 'git diff' in /Users/dev/src/acme-api
        """
        XCTAssertEqual(parseRunResult(raw: raw), .failed("no answer in the log"))
    }
}
