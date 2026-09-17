package cmd

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/gitscope"
	"github.com/1F47E/rival/internal/review"
	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/spf13/cobra"
)

var reviewCmd = &cobra.Command{
	Use:   "review [scope]",
	Short: "Run a configurable multi-model code review with consilium judge",
	Long: `Run selected code-review models in parallel with role-specific prompts,
then merge findings via a consilium judge for a unified verdict.

By default Rival runs Sol and Kimi K3. --model replaces that complete roster
for one run.

Without a scope argument, auto-detects changed files via git:
  1. Dirty files (staged + unstaged + untracked) → review those
  2. Last commit (if clean) → review files from HEAD
  3. Full project → fallback if no git changes found

With a scope argument, reviews exactly that scope.`,
	RunE: reviewAction,
}

func init() {
	reviewCmd.Flags().String("effort", "", "override all compatible model defaults: low, medium, high, ultra")
	reviewCmd.Flags().StringSliceP("model", "m", nil, "exact reviewer roster: sol, kimi-k3, grok (opt-in)")
	reviewCmd.Flags().String("workdir", ".", "working directory")
	reviewCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	rootCmd.AddCommand(reviewCmd)
}

func reviewAction(cmd *cobra.Command, args []string) error {
	effort, _ := cmd.Flags().GetString("effort")
	models, _, err := modelSelectionFlag(cmd)
	if err != nil {
		return err
	}
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")

	if effort != "" && !config.IsValidEffort(effort) {
		return fmt.Errorf("invalid effort level %q, must be one of: %v", effort, config.ValidEfforts)
	}

	// Build scope from args or auto-detect via git.
	scope := strings.Join(args, " ")
	if scope == "" {
		files := gitscope.Resolve(workdir)
		if files != "" {
			log.Info().Str("files", files).Msg("git scope: auto-detected changed files")
			scope = config.DiffReviewPreamble
			scope = strings.ReplaceAll(scope, "{FILES}", files)
			diffStat := gitscope.DiffStat(workdir)
			if diffStat != "" {
				scope = strings.ReplaceAll(scope, "{DIFFSTAT}", "\nDiff stats:\n```\n"+diffStat+"\n```\n")
			} else {
				scope = strings.ReplaceAll(scope, "{DIFFSTAT}", "")
			}
			scope += "\nFocus your review on the changed files listed above."
		} else {
			scope = "the entire project"
		}
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	ctx = review.WithCredentialWorkdir(ctx, workdir)
	scope, workdir, closeMR, err := prepareReviewScope(ctx, scope, workdir)
	if err != nil {
		return err
	}
	defer closeMR()

	groupID := uuid.New().String()

	result, err := review.RunMegaReviewWithModels(ctx, scope, effort, workdir, groupID, noQueue, models)
	if err != nil {
		return err
	}

	_, _ = fmt.Fprint(os.Stdout, review.FormatConsole(result.Output, result.Inputs, result.Threshold, result.JudgeCLI, result.JudgeModel, result.Skipped))
	return nil
}

// modelSelectionFlag distinguishes an omitted StringSlice flag from an
// explicitly empty --model=. pflag returns an empty slice for both, but the
// latter must be rejected instead of silently restoring the default roster.
func modelSelectionFlag(cmd *cobra.Command) (models []string, changed bool, err error) {
	flag := cmd.Flags().Lookup("model")
	changed = flag != nil && flag.Changed
	models, err = cmd.Flags().GetStringSlice("model")
	if err != nil {
		return nil, changed, err
	}
	if !changed {
		return nil, false, nil
	}
	if len(models) == 0 {
		return nil, true, fmt.Errorf("option --model requires a value: sol, kimi-k3, or grok")
	}
	for _, model := range models {
		if strings.TrimSpace(model) == "" {
			return nil, true, fmt.Errorf("model selector cannot be empty")
		}
	}
	return models, true, nil
}
