package review

import (
	"os"
	"path/filepath"
	"testing"
)

// codexStyleReviewerLog mimics a Codex reviewer log: the CLI echoes the full
// input prompt (which contains the JSON *schema example* from the output
// contract) before streaming its real answer last.
const codexStyleReviewerLog = `OpenAI Codex
user
Review scope: rival/

## Output Format
Return JSON only.

` + "```json" + `
{
  "summary": "1-3 sentence reviewer summary",
  "findings": [
    {
      "file": "path/to/file",
      "line": 42,
      "severity": "critical|high|medium|low",
      "title": "brief title",
      "confidence": 8
    }
  ]
}
` + "```" + `

exec /bin/zsh -lc "nl -ba main.go"
codex
{"summary":"Found a real issue.","findings":[{"file":"rival/main.go","line":10,"severity":"high","category":"bug","title":"real bug","body":"real explanation","confidence":9}]}
tokens used 1234
`

func TestParseReviewerOutput_IgnoresEchoedSchemaExample(t *testing.T) {
	out, err := ParseReviewerOutput(codexStyleReviewerLog)
	if err != nil {
		t.Fatalf("ParseReviewerOutput: %v", err)
	}
	if out.Summary != "Found a real issue." {
		t.Fatalf("summary = %q, want the real answer (not the schema example)", out.Summary)
	}
	if len(out.Findings) != 1 || out.Findings[0].File != "rival/main.go" {
		t.Fatalf("findings = %+v, want the real finding at rival/main.go", out.Findings)
	}
	if out.Findings[0].File == "path/to/file" {
		t.Fatal("parsed the schema placeholder instead of the real answer")
	}
}

// codexStyleConsiliumLog mimics a Codex consilium-judge log: it echoes the
// consilium prompt (with the schema example, including the found_by field) and
// then emits the real verdict last — sometimes twice (streamed + final).
const codexStyleConsiliumLog = `OpenAI Codex
user
# Consilium Judge

## Output Format
` + "```json" + `
{
  "summary": "1-3 sentence overall review summary",
  "findings": [{"file": "path/to/file", "line": 42, "found_by": ["sol", "kimi-k3", "claude"]}],
  "recommendation": {"status": "approve|request_changes|comment", "summary": "1-2 sentence recommendation"}
}
` + "```" + `

codex
{"summary":"Real verdict.","findings":[{"file":"rival/cmd/root.go","line":39,"severity":"medium","title":"startup scan","body":"x","confidence":10,"found_by":["kimi-k3","claude"]}],"recommendation":{"status":"request_changes","summary":"fix the races"}}
{"summary":"Real verdict.","findings":[{"file":"rival/cmd/root.go","line":39,"severity":"medium","title":"startup scan","body":"x","confidence":10,"found_by":["kimi-k3","claude"]}],"recommendation":{"status":"request_changes","summary":"fix the races"}}
`

func TestParseConsiliumOutput_IgnoresEchoedSchemaExample(t *testing.T) {
	out, err := ParseConsiliumOutput(codexStyleConsiliumLog)
	if err != nil {
		t.Fatalf("ParseConsiliumOutput: %v", err)
	}
	if out.Summary != "Real verdict." {
		t.Fatalf("summary = %q, want the real verdict (not the schema example)", out.Summary)
	}
	if out.Recommendation.Status != "request_changes" {
		t.Fatalf("recommendation = %q, want request_changes (real), got the schema placeholder", out.Recommendation.Status)
	}
	if len(out.Findings) != 1 || out.Findings[0].File != "rival/cmd/root.go" {
		t.Fatalf("findings = %+v, want the real finding", out.Findings)
	}
}

func TestParseReviewerOutput_BareObjectWithProse(t *testing.T) {
	raw := `prefix noise {"summary":"ok","findings":[]} trailing`
	out, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatalf("ParseReviewerOutput: %v", err)
	}
	if out.Summary != "ok" || len(out.Findings) != 0 {
		t.Fatalf("got %+v, want summary=ok findings=[]", out)
	}
}

func TestParseReviewerOutput_NoPayload(t *testing.T) {
	if _, err := ParseReviewerOutput("no json here at all"); err == nil {
		t.Fatal("expected error when there is no payload")
	}
}

// Unrelated JSON (e.g. a tool/telemetry event) must not be accepted as an empty
// successful parse.
func TestParseReviewerOutput_RejectsUnrelatedJSON(t *testing.T) {
	if _, err := ParseReviewerOutput(`{"event":"done","ok":true}`); err == nil {
		t.Fatal("expected error: unrelated JSON has no findings key and must be rejected")
	}
}

// If the only payload-shaped object is the echoed schema example, parsing must
// fail rather than return placeholder findings.
func TestParseReviewerOutput_RejectsOnlySchemaExample(t *testing.T) {
	schemaOnly := "```json\n" + `{"summary":"1-3 sentence reviewer summary","findings":[{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8}]}` + "\n```"
	if _, err := ParseReviewerOutput(schemaOnly); err == nil {
		t.Fatal("expected error: the schema example must not be accepted as a real answer")
	}
}

// A genuine clean review (real summary, no findings) must be accepted.
func TestParseReviewerOutput_AcceptsCleanReview(t *testing.T) {
	out, err := ParseReviewerOutput(`prose {"summary": "No issues found.", "findings": []} prose`)
	if err != nil {
		t.Fatalf("ParseReviewerOutput: %v", err)
	}
	if out.Summary != "No issues found." || len(out.Findings) != 0 {
		t.Fatalf("got %+v, want a clean review", out)
	}
}

// A real review with one stray placeholder-shaped finding must keep its real
// findings and only drop the placeholder, not be discarded wholesale.
func TestParseReviewerOutput_DropsOnlyPlaceholderFindings(t *testing.T) {
	raw := `{"summary":"real","findings":[` +
		`{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8},` +
		`{"file":"real.go","line":7,"severity":"high","category":"bug","title":"real","body":"b","confidence":9}]}`
	out, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatalf("ParseReviewerOutput: %v", err)
	}
	if len(out.Findings) != 1 || out.Findings[0].File != "real.go" {
		t.Fatalf("got findings=%+v, want only the real one kept", out.Findings)
	}
}

// A real payload nested inside a larger balanced-but-invalid region must still
// be found (the stack scanner tests every closed brace pair).
func TestParseReviewerOutput_NestedInsideInvalidRegion(t *testing.T) {
	raw := `wrapper { not valid json but balanced: {"summary":"nested real","findings":[{"file":"a.go","line":1,"severity":"high","confidence":8}]} }`
	out, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatalf("ParseReviewerOutput: %v", err)
	}
	if out.Summary != "nested real" {
		t.Fatalf("got summary=%q, want the nested real payload", out.Summary)
	}
}

// Structural detection must NOT reject a real answer merely because a finding's
// body discusses the enum literals (this happens when reviewing the parser).
func TestParseReviewerOutput_AcceptsRealFindingDiscussingEnum(t *testing.T) {
	raw := `{"summary":"real review","findings":[{"file":"rival/internal/review/parse.go","line":84,"severity":"high","category":"bug","title":"placeholder check","body":"isPlaceholderFinding compares against critical|high|medium|low which is fine","confidence":9}]}`
	out, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatalf("ParseReviewerOutput: %v", err)
	}
	if out.Summary != "real review" || len(out.Findings) != 1 || out.Findings[0].Severity != "high" {
		t.Fatalf("got %+v, want the real finding accepted despite enum text in body", out)
	}
}

// An unbalanced brace in echoed code/prose before the real answer must not stop
// the scan — the real JSON payload follows it.
func TestExtractJSON_UnbalancedBraceBeforeAnswer(t *testing.T) {
	raw := `{"summary":"schema","findings":[{"file":"path/to/file","line":42}]}
exec: showing code
func New() {           // <- unbalanced brace, never closes as JSON
    x := "a } in a string"
codex
{"summary":"real","findings":[{"file":"real.go","line":1,"confidence":9}]}`
	out, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatalf("ParseReviewerOutput: %v", err)
	}
	if out.Summary != "real" || len(out.Findings) != 1 || out.Findings[0].File != "real.go" {
		t.Fatalf("got summary=%q findings=%+v, want the real answer after the unbalanced brace", out.Summary, out.Findings)
	}
}

// Regression test against a real Codex consilium log captured in the wild: the
// CLI echoed the prompt (schema example with "path/to/file") and the cat'd
// source files (which contain braces and JSON fixtures), then emitted the real
// verdict last. The parser must return the real verdict, not the schema.
func TestParseConsiliumOutput_RealCapturedLog(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("testdata", "consilium_echoed_schema.log"))
	if err != nil {
		t.Fatalf("read fixture: %v", err)
	}
	out, err := ParseConsiliumOutput(string(data))
	if err != nil {
		t.Fatalf("ParseConsiliumOutput: %v", err)
	}
	if len(out.Findings) == 0 {
		t.Fatal("got 0 findings — parser likely returned the schema/empty, not the real verdict")
	}
	for _, f := range out.Findings {
		if f.File == "path/to/file" {
			t.Fatalf("parsed the schema placeholder (path/to/file) as a finding: %+v", f)
		}
	}
	if out.Recommendation.Status != "request_changes" {
		t.Fatalf("recommendation = %q, want the real verdict request_changes", out.Recommendation.Status)
	}
}

// A real finding with an uncertain dual category ("bug|security") must NOT be
// treated as a placeholder — only the exact contract enum literals are echoes.
func TestParsePlanOutput_KeepsRealDualCategoryFinding(t *testing.T) {
	raw := `{"summary":"real","rating":6,"findings":[` +
		`{"file":"a.go","line":3,"severity":"high","category":"bug|security",` +
		`"title":"real dual-category finding","body":"b","confidence":8}]}`
	out, err := ParsePlanOutput(raw)
	if err != nil {
		t.Fatalf("ParsePlanOutput: %v", err)
	}
	if len(out.Findings) != 1 || out.Findings[0].Title != "real dual-category finding" {
		t.Fatalf("got %+v, want the dual-category finding kept", out.Findings)
	}
}

// A partial echo of the antislop schema example — real severity, copied
// pipe-delimited category enum — must be stripped as a placeholder. This is
// the partial-echo case the file/severity literals alone do not catch.
func TestParsePlanOutput_DropsPartiallyEchoedAntislopExample(t *testing.T) {
	raw := `{"summary":"lean enough","rating":9,"findings":[` +
		`{"file":"cmd/command_antislop.go","line":10,"severity":"high",` +
		`"category":"reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni",` +
		`"title":"echoed example","body":"copied from the schema","confidence":8},` +
		`{"file":"cmd/command_antislop.go","line":20,"severity":"medium","category":"slop",` +
		`"title":"real finding","body":"a real cut","confidence":7}]}`
	out, err := ParsePlanOutput(raw)
	if err != nil {
		t.Fatalf("ParsePlanOutput: %v", err)
	}
	if len(out.Findings) != 1 || out.Findings[0].Title != "real finding" {
		t.Fatalf("got %+v, want only the real finding kept", out.Findings)
	}
}
