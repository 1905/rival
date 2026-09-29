package cmd

import (
	"github.com/spf13/cobra"
)

const claudeUsage = `Usage:
  /rival-claude 'explain the auth flow' — run any prompt with Claude
  /rival-claude -re high 'find bugs in src/main.go' — run with a higher reasoning effort
  /rival-claude review — bug-hunting review of the changed files (git auto-detect)
  /rival-claude review src/api/ — review specific scope
  /rival-claude -re high review src/api/ — review with high reasoning
  /rival-claude — show this usage info

Reasoning effort (-re): low, medium, high, xhigh.
Omitted uses efforts.claude from ~/.rival/config.yaml (built-in default: medium).`

var commandClaudeCmd = &cobra.Command{
	Use:   "claude",
	Short: "Skill-facing Claude executor",
	RunE:  commandClaudeAction,
}

func init() {
	commandClaudeCmd.Flags().String("workdir", ".", "working directory")
	commandClaudeCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	commandCmd.AddCommand(commandClaudeCmd)
}

func commandClaudeAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	return runModelCommand(claudeSpec(), workdir, noQueue)
}
