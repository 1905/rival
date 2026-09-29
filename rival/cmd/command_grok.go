package cmd

import (
	"github.com/1905/rival/internal/config"
	"github.com/spf13/cobra"
)

const grokUsage = `Usage:
  /rival-grok 'explain the auth flow' — run any prompt with Grok
  /rival-grok -re high 'find bugs in src/main.go' — pick the reasoning level
  /rival-grok review — bug-hunting review of the changed files (git auto-detect)
  /rival-grok review src/api/ — review specific scope
  /rival-grok -re high review src/api/ — review with high reasoning
  /rival-grok — show this usage info

Reasoning effort (-re): low, medium, high (levels above high clamp to high).
Omitted uses efforts.grok from ~/.rival/config.yaml (built-in: high).
Review mode runs read-only sandboxed; raw prompts can edit files in the workdir.`

var commandGrokCmd = &cobra.Command{
	Use:   config.GrokLabel,
	Short: "Skill-facing Grok executor",
	RunE:  commandGrokAction,
}

func init() {
	commandGrokCmd.Flags().String("workdir", ".", "working directory")
	commandGrokCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	commandCmd.AddCommand(commandGrokCmd)
}

func commandGrokAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	return runModelCommand(grokSpec(), workdir, noQueue)
}
