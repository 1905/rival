package cmd

import (
	"github.com/1905/rival/internal/config"
	"github.com/spf13/cobra"
)

var runClaudeCmd = &cobra.Command{
	Use:   config.ClaudeLabel,
	Short: "Run Claude",
	RunE:  runClaudeAction,
}

func init() {
	configureRunClaudeFlags(runClaudeCmd)
	runCmd.AddCommand(runClaudeCmd)
}

func configureRunClaudeFlags(cmd *cobra.Command) {
	cmd.Flags().String("effort", "", "reasoning effort override (low, medium, high, xhigh)")
	cmd.Flags().String("workdir", ".", "working directory")
	cmd.Flags().Bool("prompt-stdin", false, "read prompt from stdin")
	cmd.Flags().String("review", "", "review scope (enables review mode)")
	cmd.Flags().Bool("no-queue", false, "bypass the review queue")
}

func runClaudeAction(cmd *cobra.Command, args []string) error {
	effort, _ := cmd.Flags().GetString("effort")
	workdir, _ := cmd.Flags().GetString("workdir")
	promptStdin, _ := cmd.Flags().GetBool("prompt-stdin")
	reviewScope, _ := cmd.Flags().GetString("review")
	noQueue, _ := cmd.Flags().GetBool("no-queue")

	return runModelRun(claudeSpec(), runOptions{
		workdir:     workdir,
		noQueue:     noQueue,
		effort:      effort,
		reviewScope: reviewScope,
		isReview:    cmd.Flags().Changed("review"),
		promptStdin: promptStdin,
	})
}
