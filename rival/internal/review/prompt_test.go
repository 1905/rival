package review

import (
	"strings"
	"testing"

	"github.com/1F47E/rival/internal/config"
)

func TestBuildConsiliumPrompt_NilParsedBounded(t *testing.T) {
	bigRaw := strings.Repeat("X", 1_000_000)
	inputs := []ReviewInput{
		{CLI: "opencode", Model: config.KimiModel, RawOutput: bigRaw},
		{CLI: "codex", Model: config.GPT56SolModel, RawOutput: "small"},
	}
	prompt := BuildConsiliumPrompt(inputs, "the entire project", 6)
	if len(prompt) > 20_000 {
		t.Errorf("prompt too large: %d bytes", len(prompt))
	}
}

func TestBuildConsiliumPrompt_ParsedUsedVerbatim(t *testing.T) {
	inputs := []ReviewInput{
		{
			CLI: "codex", Model: config.GPT56SolModel,
			Parsed: &ReviewerOutput{Summary: "all good", Findings: nil},
		},
	}
	prompt := BuildConsiliumPrompt(inputs, "src/", 6)
	if !strings.Contains(prompt, "all good") {
		t.Error("parsed summary not found in prompt")
	}
}

func TestBuildConsiliumPrompt_UsesGPTModelName(t *testing.T) {
	prompt := BuildConsiliumPrompt([]ReviewInput{{
		CLI: "codex", Model: config.GPT56SolModel, Parsed: &ReviewerOutput{},
	}}, "src/", 6)
	for _, want := range []string{
		"REVIEW FROM " + config.SolLabel,
		"Allowed found_by labels for this run: " + config.SolLabel,
		`"found_by": ["` + config.SolLabel + `"]`,
	} {
		if !strings.Contains(prompt, want) {
			t.Errorf("consilium prompt missing %q", want)
		}
	}
	if strings.Contains(prompt, config.GPT56SolModel) || strings.Contains(strings.ToLower(prompt), "codex") {
		t.Fatalf("consilium prompt exposes an internal model or adapter name:\n%s", prompt)
	}
}

func TestBuildConsiliumPrompt_UsesConcreteOpencodeLabels(t *testing.T) {
	inputs := []ReviewInput{
		{CLI: "opencode", Model: config.KimiModel, Parsed: &ReviewerOutput{}},
		{CLI: "claude", Model: config.ClaudeModel, Parsed: &ReviewerOutput{}},
	}
	prompt := BuildConsiliumPrompt(inputs, "src/", 6)
	for _, want := range []string{
		"REVIEW FROM kimi-k3",
		"REVIEW FROM claude",
		`"found_by": ["kimi-k3"]`,
		"Allowed found_by labels for this run: kimi-k3, claude",
		`never the generic label "opencode"`,
	} {
		if !strings.Contains(prompt, want) {
			t.Errorf("consilium prompt missing %q", want)
		}
	}
}

func TestBuildConsiliumPrompt_FoundBySchemaMatchesExactSubset(t *testing.T) {
	prompt := BuildConsiliumPrompt([]ReviewInput{{
		CLI: "opencode", Model: config.KimiModel, Parsed: &ReviewerOutput{},
	}}, "src/", 6)
	if !strings.Contains(prompt, `"found_by": ["kimi-k3"]`) {
		t.Fatalf("single-model found_by schema does not match selection:\n%s", prompt)
	}
	if strings.Contains(prompt, `"found_by": ["claude"`) {
		t.Fatal("single-model schema contains an unselected reviewer")
	}
}

func TestFailedReviewerStub_TruncatesLongOutput(t *testing.T) {
	raw := strings.Repeat("A", 10_000)
	stub := failedReviewerStub("kimi-k3", raw)
	if len(stub) > maxDebugTail+500 {
		t.Errorf("stub too large: %d bytes", len(stub))
	}
	if !strings.Contains(stub, "failed to produce structured JSON") {
		t.Error("stub missing failure message")
	}
}
