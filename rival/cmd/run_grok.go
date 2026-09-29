package cmd

import (
	"github.com/1905/rival/internal/config"
	"github.com/spf13/cobra"
)

var runGrokCmd = &cobra.Command{
	Use:   config.GrokLabel,
	Short: "Run Grok",
	RunE:  runGrokAction,
}

func init() {
	runGrokCmd.Flags().String("effort", "", "reasoning effort override: low, medium, high (ultra clamps to high)")
	runGrokCmd.Flags().String("workdir", ".", "working directory")
	runGrokCmd.Flags().Bool("prompt-stdin", false, "read prompt from stdin")
	runGrokCmd.Flags().String("review", "", "review scope (enables review mode)")
	runGrokCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	runCmd.AddCommand(runGrokCmd)
}

func runGrokAction(cmd *cobra.Command, args []string) error {
	effort, _ := cmd.Flags().GetString("effort")
	workdir, _ := cmd.Flags().GetString("workdir")
	promptStdin, _ := cmd.Flags().GetBool("prompt-stdin")
	reviewScope, _ := cmd.Flags().GetString("review")
	noQueue, _ := cmd.Flags().GetBool("no-queue")

	return runModelRun(grokSpec(), runOptions{
		workdir:     workdir,
		noQueue:     noQueue,
		effort:      effort,
		reviewScope: reviewScope,
		isReview:    cmd.Flags().Changed("review"),
		promptStdin: promptStdin,
	})
}
