package cmd

import (
	"strings"
	"testing"

	"github.com/1F47E/rival/internal/config"
	"github.com/spf13/cobra"
)

func TestSolCommandsArePublicAndDeferToConfiguredEffort(t *testing.T) {
	if commandSolCmd.Use != config.SolLabel || commandSolCmd.Hidden {
		t.Fatalf("command metadata = use %q hidden %v", commandSolCmd.Use, commandSolCmd.Hidden)
	}
	if runSolCmd.Use != config.SolLabel || runSolCmd.Hidden {
		t.Fatalf("run metadata = use %q hidden %v", runSolCmd.Use, runSolCmd.Hidden)
	}

	effort := runSolCmd.Flags().Lookup("effort")
	if effort == nil || effort.DefValue != "" {
		t.Fatalf("run effort default = %v, want configured-default sentinel", effort)
	}
}

func TestSolUsageUsesOnlyPublicModelNaming(t *testing.T) {
	lower := strings.ToLower(solUsage)
	if !strings.Contains(lower, "/rival-sol") {
		t.Fatal("usage must name /rival-sol")
	}
	for _, hidden := range []string{"codex", "gpt-5.6", "rival-gpt"} {
		if strings.Contains(lower, hidden) {
			t.Fatalf("usage exposes hidden runtime/model name %q", hidden)
		}
	}
	for _, want := range []string{"built-in: high", "ultra"} {
		if !strings.Contains(lower, want) {
			t.Fatalf("usage missing %q", want)
		}
	}
}

func TestClaudeCommandsArePublic(t *testing.T) {
	if commandClaudeCmd.Use != config.ClaudeLabel || commandClaudeCmd.Hidden {
		t.Fatalf("command metadata = use %q hidden %v", commandClaudeCmd.Use, commandClaudeCmd.Hidden)
	}
	if runClaudeCmd.Use != config.ClaudeLabel || runClaudeCmd.Hidden {
		t.Fatalf("run metadata = use %q hidden %v", runClaudeCmd.Use, runClaudeCmd.Hidden)
	}
	if lower := strings.ToLower(claudeUsage); !strings.Contains(lower, "/rival-claude") || !strings.Contains(lower, "built-in default: medium") {
		t.Fatalf("claude usage lacks public name or effort fallback: %q", lower)
	}
}

func TestModelCommandParentsRejectUnknownRunnerNames(t *testing.T) {
	for _, parent := range []*cobra.Command{runCmd, commandCmd} {
		if err := parent.ValidateArgs([]string{"retired-runner"}); err == nil {
			t.Fatalf("%s accepted an unknown runner name", parent.CommandPath())
		}
		if err := parent.ValidateArgs(nil); err != nil {
			t.Fatalf("%s rejected an empty invocation: %v", parent.CommandPath(), err)
		}
	}
}
