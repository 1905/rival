package cmd

import (
	"github.com/1F47E/rival/internal/config"
	"github.com/spf13/cobra"
)

var runSolCmd = &cobra.Command{
	Use:   config.SolLabel,
	Short: "Run Sol",
	RunE:  runGPT56SolAction,
}

func init() {
	configureRunGPT56SolFlags(runSolCmd)
	runCmd.AddCommand(runSolCmd)
}

func configureRunGPT56SolFlags(cmd *cobra.Command) {
	cmd.Flags().String("effort", "", "reasoning effort override: low, medium, high, ultra")
	cmd.Flags().String("workdir", ".", "working directory")
	cmd.Flags().Bool("prompt-stdin", false, "read prompt from stdin")
	cmd.Flags().String("review", "", "review scope (enables review mode)")
	cmd.Flags().Bool("no-queue", false, "bypass the review queue")
}

func runGPT56SolAction(cmd *cobra.Command, args []string) error {
	effort, _ := cmd.Flags().GetString("effort")
	workdir, _ := cmd.Flags().GetString("workdir")
	promptStdin, _ := cmd.Flags().GetBool("prompt-stdin")
	reviewScope, _ := cmd.Flags().GetString("review")
	noQueue, _ := cmd.Flags().GetBool("no-queue")

	return runModelRun(solSpec(), runOptions{
		workdir:     workdir,
		noQueue:     noQueue,
		effort:      effort,
		reviewScope: reviewScope,
		isReview:    cmd.Flags().Changed("review"),
		promptStdin: promptStdin,
	})
}
