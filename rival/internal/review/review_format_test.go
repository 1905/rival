package review

import (
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
)

func reviewFixture() *ReviewerOutput {
	return &ReviewerOutput{
		Summary: "One real bug in the stop path.",
		Findings: []ReviewerFinding{
			{File: "rival/internal/dashboard/list.go", Line: 12, Severity: "low", Title: "stale label", Category: "ux", Confidence: 4},
			{File: "rival/internal/dashboard/model.go", Line: 597, Severity: "high", Title: "Stop can signal a reused PID",
				Body:       "Confirmation rechecks only the cached status; PIDStart is never checked.",
				Suggestion: "compare the procinfo start time to PIDStart before signalling.",
				Category:   "bug", Confidence: 9},
		},
	}
}

func TestFormatReviewConsoleLayout(t *testing.T) {
	got := FormatReviewConsole(reviewFixture(), "codex (gpt-6-astra)", "rival/internal/dashboard/", "/tmp/s.log")
	want := `
═══ RIVAL REVIEW ═══

Model: codex (gpt-6-astra)
Scope: rival/internal/dashboard/

Summary: One real bug in the stop path.

1. [high] Stop can signal a reused PID — rival/internal/dashboard/model.go:597
   Confirmation rechecks only the cached status; PIDStart is never checked.
   Fix: compare the procinfo start time to PIDStart before signalling.
   (bug, confidence 9)

Low confidence (1):
- [low] stale label — rival/internal/dashboard/list.go:12 (confidence 4)

Findings: 1 total — 0 crit, 1 high, 0 med, 0 low
Log: /tmp/s.log
`
	if got != want {
		t.Errorf("layout mismatch\n got:\n%s\nwant:\n%s", got, want)
	}
}

func TestFormatReviewConsoleOrdersBySeverityThenConfidence(t *testing.T) {
	out := &ReviewerOutput{Summary: "s", Findings: []ReviewerFinding{
		{File: "m.go", Severity: "medium", Title: "med", Confidence: 9},
		{File: "h1.go", Severity: "high", Title: "high-7", Confidence: 7},
		{File: "c.go", Severity: "critical", Title: "crit", Confidence: 6},
		{File: "h2.go", Severity: "high", Title: "high-9", Confidence: 9},
	}}
	got := FormatReviewConsole(out, "m", "s", "")
	order := []string{"1. [crit] crit", "2. [high] high-9", "3. [high] high-7", "4. [med] med"}
	last := -1
	for _, o := range order {
		i := strings.Index(got, o)
		if i <= last {
			t.Fatalf("%q out of order:\n%s", o, got)
		}
		last = i
	}
	if !strings.Contains(got, "Findings: 4 total — 1 crit, 2 high, 1 med, 0 low\n") {
		t.Errorf("tally wrong:\n%s", got)
	}
	if strings.Contains(got, "Log:") {
		t.Errorf("empty log path must print no Log line:\n%s", got)
	}
}

func TestFormatReviewConsoleEmpty(t *testing.T) {
	got := FormatReviewConsole(&ReviewerOutput{Summary: "No issues found."}, "m", "s", "/tmp/x.log")
	if !strings.Contains(got, "No issues found.\n") || strings.Contains(got, "Findings:") {
		t.Errorf("empty review layout wrong:\n%s", got)
	}
	if !strings.HasSuffix(got, "Log: /tmp/x.log\n") {
		t.Errorf("Log line missing:\n%s", got)
	}
}

// Only low-confidence findings: they are shown, not dropped, and the main
// list says why it is empty.
func TestFormatReviewConsoleOnlyLowConfidence(t *testing.T) {
	out := &ReviewerOutput{Summary: "s", Findings: []ReviewerFinding{
		{File: "a.go", Line: 3, Severity: "medium", Title: "maybe", Confidence: 5},
	}}
	got := FormatReviewConsole(out, "m", "s", "")
	for _, want := range []string{
		"No findings at confidence 6 or higher.",
		"Low confidence (1):\n- [med] maybe — a.go:3 (confidence 5)",
		"Findings: 0 total",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q:\n%s", want, got)
		}
	}
}

func TestFormatReviewConsoleJoinsMultiLineScope(t *testing.T) {
	got := FormatReviewConsole(&ReviewerOutput{Summary: "s"}, "m", "a.go\nb.go\n", "")
	if !strings.Contains(got, "Scope: a.go, b.go\n") {
		t.Errorf("scope not joined:\n%s", got)
	}
}

func TestFormatReviewResultModelLine(t *testing.T) {
	got := FormatReviewResult(reviewFixture(), `{"summary":"x","findings":[]}`, "codex", config.CodexModel, "src/", "/tmp/l.log")
	if !strings.Contains(got, "═══ RIVAL REVIEW ═══") || !strings.Contains(got, "Model: codex (gpt-6-astra)\n") {
		t.Errorf("formatted review missing header or model line:\n%s", got)
	}
}

func TestFormatReviewResultFallsBackToRawLog(t *testing.T) {
	for _, tc := range []struct {
		name   string
		parsed *ReviewerOutput
		raw    string
	}{
		{"nil", nil, "the model wrote prose instead"},
		{"empty summary", &ReviewerOutput{Summary: "  "}, "the model wrote prose instead"},
		{"echoed example", &ReviewerOutput{Summary: "No issues found."},
			"user\n" + BuildReviewerPrompt("src/", config.PromptBugHunter) + "\ntokens used: 10\n"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := FormatReviewResult(tc.parsed, tc.raw, "codex", config.CodexModel, "src/", "/tmp/l.log")
			if !strings.Contains(got, "═══ RIVAL REVIEW — UNPARSED OUTPUT ═══") {
				t.Errorf("no UNPARSED header:\n%s", got)
			}
			if !strings.Contains(got, "Problem: ") || !strings.Contains(got, "Log: /tmp/l.log") {
				t.Errorf("problem or log line missing:\n%s", got)
			}
			if strings.Contains(got, "No issues found.\n\nLog") {
				t.Errorf("unusable output rendered as a clean review:\n%s", got)
			}
		})
	}
	got := FormatReviewResult(nil, "the model wrote prose instead", "codex", config.CodexModel, "src/", "")
	if !strings.Contains(got, "the model wrote prose instead") {
		t.Errorf("raw log dropped:\n%s", got)
	}
}

// Codex writes the whole prompt into its log, so the bug-hunter marker is in
// every codex log. A genuine clean answer after the echoed prompt must pass.
func TestBugHunterCleanReviewAfterEchoedPromptIsAccepted(t *testing.T) {
	raw := "user\n" + BuildReviewerPrompt("src/", config.PromptBugHunter) +
		"\ncodex\n{\"summary\": \"No issues found.\", \"findings\": []}\ntokens used: 10\n"
	parsed, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatal(err)
	}
	if err := validateReviewResult(parsed, raw); err != nil {
		t.Errorf("genuine clean codex review rejected: %v", err)
	}
}

// The echo-only codex log: the prompt and nothing after it.
func TestBugHunterEchoOnlyIsRejected(t *testing.T) {
	raw := "user\n" + BuildReviewerPrompt("src/", config.PromptBugHunter) + "\ntokens used: 10\n"
	parsed, err := ParseReviewerOutput(raw)
	if err != nil {
		t.Fatalf("the echoed clean example should parse: %v", err)
	}
	if err := validateReviewResult(parsed, raw); err == nil {
		t.Error("an echo-only log was accepted as a clean review")
	}
}

func TestSeverityTallyCountsUnknownAsLow(t *testing.T) {
	got := severityTally([]ReviewerFinding{{Severity: "critical"}, {Severity: "HIGH"}, {Severity: "spicy"}})
	if got != "Findings: 3 total — 1 crit, 1 high, 0 med, 1 low" {
		t.Errorf("tally = %q", got)
	}
}

func TestSortedFindingsDoesNotMutateInput(t *testing.T) {
	in := []ReviewerFinding{{Title: "l", Severity: "low"}, {Title: "c", Severity: "critical"}}
	out := sortedFindings(in)
	if in[0].Title != "l" || out[0].Title != "c" {
		t.Errorf("in=%v out=%v", in, out)
	}
}
