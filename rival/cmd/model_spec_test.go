package cmd

import (
	"testing"

	"github.com/1F47E/rival/internal/config"
)

// The command word and the display label differ for K3, so one field cannot
// serve both. Error text and logs use the label; cobra uses the command name.
func TestSpecLabelsAndCommandNames(t *testing.T) {
	tests := []struct {
		spec        modelSpec
		commandName string
		label       string
		cli         string
	}{
		{solSpec(), "sol", config.SolLabel, "codex"},
		{claudeSpec(), "claude", config.ClaudeLabel, "claude"},
		{k3Spec(), "k3", config.K3Label, "opencode"},
		{grokSpec(), "grok", config.GrokLabel, config.GrokLabel},
	}
	for _, tt := range tests {
		t.Run(tt.commandName, func(t *testing.T) {
			if tt.spec.commandName != tt.commandName {
				t.Errorf("command name = %q, want %q", tt.spec.commandName, tt.commandName)
			}
			if got := tt.spec.label(); got != tt.label {
				t.Errorf("label = %q, want %q", got, tt.label)
			}
			if tt.spec.cli != tt.cli {
				t.Errorf("session cli = %q, want %q", tt.spec.cli, tt.cli)
			}
		})
	}
}

// K3 is pinned to the only level its provider supports, whatever was asked.
func TestK3EffortIsPinnedToMax(t *testing.T) {
	for _, requested := range []string{"", "low", "high", "xhigh", "ultra"} {
		got, err := k3Spec().resolveEffort(requested)
		if err != nil {
			t.Fatalf("resolveEffort(%q): %v", requested, err)
		}
		if got != "max" {
			t.Errorf("resolveEffort(%q) = %q, want max", requested, got)
		}
	}
}

// Grok exposes only low, medium and high, so wider levels clamp to high.
func TestGrokEffortClampsToItsOwnMenu(t *testing.T) {
	for _, requested := range []string{"xhigh", "ultra"} {
		got, err := grokSpec().resolveEffort(requested)
		if err != nil {
			t.Fatalf("resolveEffort(%q): %v", requested, err)
		}
		if got != "high" {
			t.Errorf("resolveEffort(%q) = %q, want high", requested, got)
		}
	}
}

// Sol receives the level verbatim: ultra is its own level, not an xhigh alias.
func TestSolEffortIsNotAliased(t *testing.T) {
	for _, requested := range []string{"xhigh", "ultra"} {
		got, err := solSpec().resolveEffort(requested)
		if err != nil {
			t.Fatalf("resolveEffort(%q): %v", requested, err)
		}
		if got != requested {
			t.Errorf("resolveEffort(%q) = %q, want it unchanged", requested, got)
		}
	}
}

// Only Claude reports an auth hint, and only from its own log.
func TestOnlyClaudeReportsAnAuthHint(t *testing.T) {
	for _, spec := range []modelSpec{solSpec(), k3Spec(), grokSpec()} {
		if hint := spec.authHint("/nonexistent.log"); hint != "" {
			t.Errorf("%s returned an auth hint %q", spec.commandName, hint)
		}
	}
}

func TestSessionModeNamesTheRun(t *testing.T) {
	if got := sessionMode(true); got != "review" {
		t.Errorf("review mode = %q", got)
	}
	if got := sessionMode(false); got != "raw" {
		t.Errorf("raw mode = %q", got)
	}
}

// An invalid effort must fail before the workflow touches the provider or
// blocks on stdin. Ordering this wrong hides the real error behind an auth
// failure, or hangs forever waiting for input that will never come.
func TestRunModelRunValidatesEffortBeforeAnythingElse(t *testing.T) {
	preflightCalled := false
	spec := solSpec()
	spec.preflight = func(string) error {
		preflightCalled = true
		return nil
	}

	// promptStdin is set but nothing is piped: if the workflow reached the
	// read, this test would block instead of returning.
	err := runModelRun(spec, runOptions{
		workdir:     t.TempDir(),
		noQueue:     true,
		effort:      "not-a-level",
		promptStdin: true,
	})
	if err == nil {
		t.Fatal("expected an invalid effort to error")
	}
	if preflightCalled {
		t.Error("preflight ran before the effort was validated")
	}
}
