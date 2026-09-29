package cmd

import (
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
	"github.com/spf13/cobra"
)

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
