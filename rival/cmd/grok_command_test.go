package cmd

import (
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/executor"
	"github.com/1905/rival/internal/parser"
)

func TestGrokCommandIsPublic(t *testing.T) {
	if commandGrokCmd.Use != config.GrokLabel || commandGrokCmd.Hidden {
		t.Fatalf("command metadata = use %q hidden %v", commandGrokCmd.Use, commandGrokCmd.Hidden)
	}
	for _, flag := range []string{"workdir", "no-queue"} {
		if commandGrokCmd.Flags().Lookup(flag) == nil {
			t.Fatalf("command grok is missing the %q flag", flag)
		}
	}
}

// The sandbox is selected by the review boolean handed to executor.RunGrok, so
// a review-mode session that reported raw would run grok unsandboxed with
// --yolo. Pin both the mode derivation and the boolean that follows from it.
func TestGrokSessionModeDrivesReviewSandbox(t *testing.T) {
	tests := []struct {
		name       string
		raw        string
		wantMode   string
		wantReview bool
	}{
		{"plain prompt", "explain the auth flow", "raw", false},
		{"review without scope", "review", "review", true},
		{"review with scope", "review src/api/", "review", true},
		{"review with effort", "-re high review src/api/", "review", true},
		{"prompt with effort", "-re low explain the auth flow", "raw", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			parsed, err := parser.ParseGrokArgs(tt.raw)
			if err != nil {
				t.Fatalf("ParseGrokArgs(%q) error: %v", tt.raw, err)
			}

			mode := sessionMode(parsed.IsReview)
			if mode != tt.wantMode {
				t.Fatalf("mode = %q, want %q", mode, tt.wantMode)
			}
			if review := mode == "review"; review != tt.wantReview {
				t.Fatalf("review sandbox = %v, want %v", review, tt.wantReview)
			}
		})
	}
}

func TestGrokUsesConfiguredGrokEffortDefault(t *testing.T) {
	parsed, err := parser.ParseGrokArgs("review")
	if err != nil {
		t.Fatalf("ParseGrokArgs error: %v", err)
	}
	effort, err := config.ResolveEffort(config.GrokModel, parsed.Effort, config.DefaultReviewEffort)
	if err != nil {
		t.Fatalf("ResolveEffort error: %v", err)
	}
	if effort != "high" {
		t.Fatalf("effort = %q, want high", effort)
	}
}

func TestRunGrokCommandFlags(t *testing.T) {
	if runGrokCmd.Use != config.GrokLabel || runGrokCmd.Hidden {
		t.Fatalf("run grok metadata = use %q hidden %v", runGrokCmd.Use, runGrokCmd.Hidden)
	}
	for _, flag := range []string{"effort", "workdir", "prompt-stdin", "review", "no-queue"} {
		if runGrokCmd.Flags().Lookup(flag) == nil {
			t.Fatalf("run grok is missing the %q flag", flag)
		}
	}
	// The flag accepts rival's full ladder (config.IsValidEffort), so the help
	// must admit that ultra is taken and clamped rather than rejected.
	if got := runGrokCmd.Flags().Lookup("effort").Usage; got != "reasoning effort override: low, medium, high (ultra clamps to high)" {
		t.Fatalf("effort help = %q", got)
	}
	if got := runGrokCmd.Flags().Lookup("workdir").DefValue; got != "." {
		t.Fatalf("workdir default = %q, want %q", got, ".")
	}

	var found bool
	for _, sub := range runCmd.Commands() {
		if sub == runGrokCmd {
			found = true
			break
		}
	}
	if !found {
		t.Fatal("run grok is not registered under `rival run`")
	}
}

// The session must record the effort grok is actually handed. rival's ladder
// goes past what grok exposes, so an ultra request lands as high — recording
// "ultra" would report a reasoning level that never ran.
func TestGrokEffortRecordedAfterClamp(t *testing.T) {
	cases := []struct{ resolved, want string }{
		{"low", "low"},
		{"medium", "medium"},
		{"high", "high"},
		{"ultra", "high"},
		{"xhigh", "high"},
		{"minimal", "low"},
	}

	for _, tc := range cases {
		t.Run(tc.resolved, func(t *testing.T) {
			got, err := executor.GrokEffort(tc.resolved)
			if err != nil {
				t.Fatalf("GrokEffort(%q): %v", tc.resolved, err)
			}
			if got != tc.want {
				t.Fatalf("session-bound effort = %q, want %q", got, tc.want)
			}
		})
	}
}

// ResolveEffort feeding straight into the clamp is the exact chain both
// `rival command grok` and `rival run grok` use before session.NewQueued.
func TestGrokResolvedUltraEffortIsSentAsHigh(t *testing.T) {
	resolved, err := config.ResolveEffort(config.GrokModel, "ultra", config.DefaultReviewEffort)
	if err != nil {
		t.Fatalf("ResolveEffort error: %v", err)
	}
	sent, err := executor.GrokEffort(resolved)
	if err != nil {
		t.Fatalf("GrokEffort error: %v", err)
	}
	if sent != "high" {
		t.Fatalf("ultra was sent as %q, want high", sent)
	}
}

func TestGrokUsageUsesOnlyPublicNaming(t *testing.T) {
	lower := strings.ToLower(grokUsage)
	if !strings.Contains(lower, "/rival-grok") {
		t.Fatal("usage must name /rival-grok")
	}
	if !strings.Contains(lower, "built-in: high") {
		t.Fatalf("usage missing the built-in effort fallback: %q", lower)
	}
}
