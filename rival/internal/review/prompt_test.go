package review

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
)

// Both code-review lenses carry the one rubric, the field and the drop rule.
func TestReviewerPromptsCarryRubricAndFailureScenario(t *testing.T) {
	for _, kind := range []config.PromptKind{config.PromptBugHunter, config.PromptSecurity} {
		prompt := BuildReviewerPrompt("src/", kind)
		for _, want := range []string{
			severityRubric,
			`"failure_scenario": "input/state that triggers it → the wrong result"`,
			"Each finding needs a concrete failure_scenario: the input or state that triggers it and the wrong result. If you cannot state one, drop the finding.",
			cleanReviewExampleLine,
			`"summary": "1-3 sentence reviewer summary"`,
		} {
			if !strings.Contains(prompt, want) {
				t.Errorf("kind %v prompt is missing %q", kind, want)
			}
		}
	}
	sec := BuildReviewerPrompt("src/", config.PromptSecurity)
	if !strings.Contains(sec, "Put the attack (what the attacker controls,\nwhat they reach, what they get) in failure_scenario.") {
		t.Error("security prompt does not put the attack in failure_scenario")
	}
}

// The field sits between body and suggestion in the contract.
func TestReviewerContractOrdersFailureScenario(t *testing.T) {
	c := reviewerJSONContract()
	body := strings.Index(c, `"body"`)
	scen := strings.Index(c, `"failure_scenario"`)
	fix := strings.Index(c, `"suggestion"`)
	if body < 0 || body >= scen || scen >= fix {
		t.Errorf("failure_scenario not between body and suggestion: body=%d scenario=%d suggestion=%d", body, scen, fix)
	}
}

// A user override replaces the lens text but still gets the contract, and the
// rubric is not injected into it.
func TestBugHunterOverrideKeepsContractWithoutRubric(t *testing.T) {
	home := t.TempDir()
	t.Cleanup(config.LoadUserConfig)
	t.Setenv("HOME", home)
	dir := filepath.Join(home, ".rival")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	cfg := "roles:\n  bug_hunter: \"CUSTOM LENS\\n\"\n"
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(cfg), 0o600); err != nil {
		t.Fatal(err)
	}
	config.LoadUserConfig()
	if err := config.UserConfigError(); err != nil {
		t.Fatalf("load config: %v", err)
	}

	got := BuildReviewerPrompt("X", config.PromptBugHunter)
	want := "Review scope: X\n\nCUSTOM LENS\n" + reviewerJSONContract()
	if got != want {
		t.Errorf("override prompt mismatch\n got:\n%s\nwant:\n%s", got, want)
	}
	if !strings.Contains(got, `"failure_scenario"`) {
		t.Error("override prompt lost failure_scenario in the contract")
	}
	if strings.Contains(got, "Severity:\n- critical") {
		t.Error("the rubric was injected into the override")
	}
}

func TestParseReviewerOutputFailureScenario(t *testing.T) {
	raw := `{"summary":"one bug","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"t","body":"b","failure_scenario":"empty list → index panic","suggestion":"s","confidence":8}]}`
	out, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatal(err)
	}
	if got := out.Findings[0].FailureScenario; got != "empty list → index panic" {
		t.Errorf("FailureScenario = %q", got)
	}
	enc, err := json.Marshal(out)
	if err != nil {
		t.Fatal(err)
	}
	back, err := ParseReviewerOutput(string(enc))
	if err != nil {
		t.Fatal(err)
	}
	if back.Findings[0] != out.Findings[0] {
		t.Errorf("round trip changed the finding: %+v vs %+v", back.Findings[0], out.Findings[0])
	}

	old := `{"summary":"one bug","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"t","body":"b","suggestion":"s","confidence":8}]}`
	out, err = ParseReviewerOutput(old)
	if err != nil {
		t.Fatalf("old payload without failure_scenario failed to parse: %v", err)
	}
	if len(out.Findings) != 1 || out.Findings[0].FailureScenario != "" {
		t.Errorf("old payload parsed wrong: %+v", out.Findings)
	}
	enc, _ = json.Marshal(out.Findings[0])
	if strings.Contains(string(enc), "failure_scenario") {
		t.Error("an empty failure_scenario is not omitted")
	}
}

func TestFormattersShowScenarioOnlyWhenSet(t *testing.T) {
	with := func(s string) *ReviewerOutput {
		return &ReviewerOutput{Summary: "x", Findings: []ReviewerFinding{{
			File: "a.go", Line: 3, Severity: "high", Category: "bug", Title: "t",
			Body: "the body", FailureScenario: s, Suggestion: "the fix", Confidence: 9,
		}}}
	}
	formatters := map[string]func(*ReviewerOutput) string{
		"review":   func(o *ReviewerOutput) string { return FormatReviewConsole(o, "m", "src/", "") },
		"security": func(o *ReviewerOutput) string { return FormatSecurityConsole(o, "m", "src/") },
	}
	for name, format := range formatters {
		got := format(with("nil map → panic on write"))
		if !strings.Contains(got, "   the body\n   Scenario: nil map → panic on write\n   Fix: the fix\n") {
			t.Errorf("%s: Scenario line missing or misplaced:\n%s", name, got)
		}
		for _, empty := range []string{"", "   "} {
			if got := format(with(empty)); strings.Contains(got, "Scenario:") {
				t.Errorf("%s: Scenario shown for %q:\n%s", name, empty, got)
			}
		}
	}
}

func TestNoPromptUsesAPersona(t *testing.T) {
	t.Setenv("HOME", t.TempDir())

	prompts := map[string]string{
		"plan":     config.PlanReviewPrompt,
		"antislop": config.AntislopCodePrompt,
		"bug":      BuildReviewerPrompt("x", config.PromptBugHunter),
		"security": BuildReviewerPrompt("x", config.PromptSecurity),
	}
	for name, p := range prompts {
		lower := strings.ToLower(p)
		for _, banned := range []string{"ruthless", "senior staff"} {
			if strings.Contains(lower, banned) {
				t.Errorf("%s prompt contains persona text %q", name, banned)
			}
		}
	}
}

func TestPlanPromptVerifiesCodeClaims(t *testing.T) {
	if !strings.Contains(config.PlanReviewPrompt, "open the repo and check them") {
		t.Error("PlanReviewPrompt lacks the repo-verification instruction")
	}
	if strings.Contains(config.PlanReviewPrompt, "(as described)") {
		t.Error("PlanReviewPrompt still judges the system only as described")
	}
}

// A clean bug-hunter review of code that quotes the prompts (a reviewer reading
// prompt.go logs "## Role: Security Reviewer" in tool output) must not be
// mistaken for an echoed prompt. Found by the 2026-09-26 smoke review.
func TestCleanReviewQuotingPromptMarkersIsNotAnEcho(t *testing.T) {
	answer := `{"summary": "No issues found.", "findings": []}`
	for _, raw := range []string{
		"user\n" + BuildReviewerPrompt("x", config.PromptBugHunter) + "\nexec cat prompt.go\n## Role: Security Reviewer\nWork through every class below\n" + cleanReviewExampleLine + "\ncodex\n" + answer,
		"tool: ## Role: Security Reviewer\n" + answer,
	} {
		out, err := ParseReviewerOutput(raw)
		if err != nil {
			t.Fatalf("parse: %v", err)
		}
		if err := validateReviewResult(out, raw); err != nil {
			t.Fatalf("genuine clean review rejected: %v", err)
		}
	}
}

// The prompt reflected back with nothing after its example is still an echo,
// for both prompt kinds.
func TestPromptEchoWithoutAnswerIsRejected(t *testing.T) {
	for _, kind := range []config.PromptKind{config.PromptBugHunter, config.PromptSecurity} {
		raw := BuildReviewerPrompt("x", kind)
		out, err := ParseReviewerOutput(raw)
		if err != nil {
			continue // the echo did not even parse: already UNPARSED
		}
		if validateReviewResult(out, raw) == nil {
			t.Fatalf("kind %v: echoed prompt accepted as a review", kind)
		}
	}
}
