package cmd

import (
	"strings"
	"testing"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/parser"
)

func TestAntislopStdinGrammar(t *testing.T) {
	tests := []struct {
		name   string
		raw    string
		effort string
		models []string
	}{
		{"empty input is auto scope", "", "", nil},
		{"options before scope", "-re high -m claude src/api/", "high", []string{"claude"}},
		{"scope with model list", "-m sol,claude src/", "", []string{"sol", "claude"}},
		{"escaped dash scope", "-- -weird/dir", "", nil},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			parsed, err := parser.ParseReviewArgs(tt.raw)
			if err != nil {
				t.Fatalf("ParseReviewArgs: %v", err)
			}
			if parsed.Effort != tt.effort {
				t.Fatalf("effort: got %q, want %q", parsed.Effort, tt.effort)
			}
			if len(parsed.Models) != len(tt.models) {
				t.Fatalf("models: got %v, want %v", parsed.Models, tt.models)
			}
		})
	}
}

func TestAntislopDefaultModelIsCodexOnly(t *testing.T) {
	clis, err := parsePlanModels(defaultAntislopModels)
	if err != nil {
		t.Fatalf("parsePlanModels: %v", err)
	}
	if len(clis) != 1 || clis[0] != "codex" {
		t.Fatalf("got %v, want codex only", clis)
	}
}

func TestBuildAntislopCodePromptExplicitScope(t *testing.T) {
	prompt, target, display := buildAntislopCodePrompt("src/api/", false, t.TempDir())
	if !strings.Contains(prompt, "Review scope: src/api/") {
		t.Fatalf("scope not substituted:\n%s", prompt[:200])
	}
	if strings.Contains(prompt, "{SCOPE}") {
		t.Fatal("placeholder left in prompt")
	}
	if target != "src/api/" || display != "src/api/" {
		t.Fatalf("got target=%q display=%q", target, display)
	}
}

func TestBuildAntislopCodePromptNoChangesFallsBackToProject(t *testing.T) {
	// A fresh temp dir is not a git repo, so auto-detect finds nothing.
	prompt, target, display := buildAntislopCodePrompt("the entire project", true, t.TempDir())
	if !strings.Contains(prompt, "Review scope: the entire project") {
		t.Fatalf("full-project fallback missing:\n%s", prompt[:200])
	}
	if target != "the entire project" || display != "the entire project" {
		t.Fatalf("got target=%q display=%q", target, display)
	}
	if !strings.Contains(prompt, config.AntislopCodePrompt[strings.Index(config.AntislopCodePrompt, "Rules:"):][:20]) {
		t.Fatal("prompt does not end with the antislop template")
	}
}
