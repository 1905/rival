package cmd

import (
	"github.com/spf13/cobra"
)

const k3Usage = `Usage:
  echo 'explain the auth flow' | rival command k3
  echo 'review' | rival command k3
  echo 'review src/api/' | rival command k3
  rival command k3 < prompt.txt

Note: k3 runs Kimi K3 (moonshotai/kimi-k3 via opencode), a thinking-only model
pinned to max reasoning — the -re flag accepts low|medium|high|xhigh|ultra|max
and ignores the value. Needs MOONSHOT_API_KEY in the project .env (or exported).
Review mode runs read-only sandboxed (same profile as the other reviewers);
raw prompts run full auto and can edit files and run commands in the workdir.`

var commandK3Cmd = &cobra.Command{
	Use:   "k3",
	Short: "Run Kimi K3 prompts from stdin",
	RunE:  commandK3Action,
}

func init() {
	commandK3Cmd.Flags().String("workdir", ".", "working directory")
	commandK3Cmd.Flags().Bool("no-queue", false, "bypass the review queue")
	commandCmd.AddCommand(commandK3Cmd)
}

func commandK3Action(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	return runModelCommand(k3Spec(), workdir, noQueue)
}
