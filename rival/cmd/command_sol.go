package cmd

import (
	"github.com/1F47E/rival/internal/config"
	"github.com/spf13/cobra"
)

const solUsage = `Usage:
  /rival-sol 'explain the auth flow' — run any prompt with Sol
  /rival-sol -re ultra 'find bugs in src/main.go' — use ultra reasoning
  /rival-sol review — ruthless code review of the entire project
  /rival-sol review src/api/ — review specific scope
  /rival-sol -re ultra review src/api/ — review with ultra reasoning
  /rival-sol — show this usage info

Reasoning effort (-re): low, medium, high, ultra.
Omitted uses efforts.sol from ~/.rival/config.yaml (built-in: high).`

var commandSolCmd = &cobra.Command{
	Use:   config.SolLabel,
	Short: "Skill-facing Sol executor",
	RunE:  commandGPT56SolAction,
}

func init() {
	configureCommandGPT56SolFlags(commandSolCmd)
	commandCmd.AddCommand(commandSolCmd)
}

func configureCommandGPT56SolFlags(cmd *cobra.Command) {
	cmd.Flags().String("workdir", ".", "working directory")
	cmd.Flags().Bool("no-queue", false, "bypass the review queue")
}

func commandGPT56SolAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	return runModelCommand(solSpec(), workdir, noQueue)
}
