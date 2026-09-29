package cmd

import (
	"github.com/1905/rival/internal/config"
	"github.com/spf13/cobra"
)

const codexUsage = `Usage:
  /rival-codex 'explain the auth flow' — run any prompt with Codex
  /rival-codex -re high 'find bugs in src/main.go' — run with a different reasoning effort (default xhigh)
  /rival-codex review — bug-hunting review of the changed files (git auto-detect)
  /rival-codex review src/api/ — review specific scope
  /rival-codex -re high review src/api/ — review with high reasoning
  /rival-codex — show this usage info

Reasoning effort (-re): low, medium, high, xhigh, ultra.
Omitted uses efforts.codex from ~/.rival/config.yaml (built-in: xhigh).`

var commandCodexCmd = &cobra.Command{
	Use:   config.CodexLabel,
	Short: "Skill-facing Codex executor",
	RunE:  commandCodexAction,
}

func init() {
	configureCommandCodexFlags(commandCodexCmd)
	commandCmd.AddCommand(commandCodexCmd)
}

func configureCommandCodexFlags(cmd *cobra.Command) {
	cmd.Flags().String("workdir", ".", "working directory")
	cmd.Flags().Bool("no-queue", false, "bypass the review queue")
}

func commandCodexAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	return runModelCommand(codexSpec(), workdir, noQueue)
}
