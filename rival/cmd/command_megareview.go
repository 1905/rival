package cmd

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"

	"github.com/1F47E/rival/internal/parser"
	"github.com/1F47E/rival/internal/review"
	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/spf13/cobra"
)

const megareviewUsage = `Usage:
  /rival-review — review changed files with Astra
  /rival-review -m astra src/api/ — review a scope with Astra only
  /rival-review -m k3 src/api/ — review a scope with Kimi K3 only
  /rival-review -m astra,k3 src/api/ — use exactly those two models
  /rival-review -m grok src/api/ — review a scope with grok only (opt-in)
  /rival-review -re ultra src/api/ — override compatible model defaults

Models (-m/--model): astra, kimi-k3 (k3), grok
grok is opt-in: it is never added implicitly, only when named.
An explicit model list replaces the default Astra roster.
Reasoning effort (-re/--effort): low, medium, high, ultra; omitted uses ~/.rival/config.yaml model defaults`

var commandMegareviewCmd = &cobra.Command{
	Use:   "megareview",
	Short: "Run a configurable multi-model review with consilium judge",
	RunE:  commandMegareviewAction,
}

func init() {
	commandMegareviewCmd.Flags().StringSliceP("model", "m", nil, "exact reviewer roster (normally supplied through skill arguments on stdin)")
	commandMegareviewCmd.Flags().String("workdir", ".", "working directory")
	commandMegareviewCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	commandCmd.AddCommand(commandMegareviewCmd)
}

func commandMegareviewAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	nativeModels, nativeModelsSet, modelFlagErr := modelSelectionFlag(cmd)
	if modelFlagErr != nil {
		_, _ = fmt.Fprintln(os.Stdout, modelFlagErr.Error())
		return &ExitCodeError{Code: 1, Err: modelFlagErr}
	}

	if stat, statErr := os.Stdin.Stat(); statErr == nil && (stat.Mode()&os.ModeCharDevice) != 0 {
		_, _ = fmt.Fprintln(os.Stdout, megareviewUsage)
		return nil
	}

	raw, err := io.ReadAll(os.Stdin)
	if err != nil {
		return fmt.Errorf("read stdin: %w", err)
	}

	parsed, err := parser.ParseReviewArgs(string(raw))
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	if parsed.IsEmpty {
		_, _ = fmt.Fprintln(os.Stdout, megareviewUsage)
		return nil
	}
	if nativeModelsSet && len(parsed.Models) > 0 {
		err := fmt.Errorf("model selection was provided both as --model command flags and in review arguments; use one form")
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}
	if nativeModelsSet {
		parsed.Models = nativeModels
	}

	// Build scope.
	scope := parsed.ReviewScope
	if parsed.AutoScope {
		preamble, files := buildDiffPreamble(workdir)
		if files != "" {
			log.Info().Str("files", files).Msg("git scope: auto-detected changed files")
			scope = preamble + "\nFocus your review on the changed files listed above."
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

	result, err := review.RunMegaReviewWithModels(ctx, scope, parsed.Effort, workdir, groupID, noQueue, parsed.Models)
	if err != nil {
		return err
	}

	_, _ = fmt.Fprint(os.Stdout, review.FormatConsole(result.Output, result.Inputs, result.Threshold, result.JudgeCLI, result.JudgeModel, result.Skipped))
	return nil
}
