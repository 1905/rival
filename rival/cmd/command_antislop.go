package cmd

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/parser"
	"github.com/1F47E/rival/internal/review"
	"github.com/1F47E/rival/internal/session"
	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
	"github.com/spf13/cobra"
)

const antislopUsage = `Usage:
  /rival-antislop — quality-only review of the changed files (git auto-detect)
  /rival-antislop src/api/ — review a specific scope
  /rival-antislop -m fable -re high src/ — pick model and reasoning effort
  rival command antislop --help — show native command options

Antislop hunts slop and over-engineering — reuse/DRY, simplification,
efficiency, altitude, backward-compat hoarding, library reinvention, comment
and wrapper slop — and returns a leanness rating (1-10) plus a cut list. It
never reports bugs; use the code review commands for that.

Input is a code-review scope. "--" ends option parsing and takes the rest
verbatim, so a scope beginning with a dash is still reviewable. Default model
is astra; -m accepts astra and fable (comma-separated). Default reasoning effort
is high; override with -re/--effort or per model in ~/.rival/config.yaml.`

var defaultAntislopModels = []string{config.AstraLabel}

var commandAntislopCmd = &cobra.Command{
	Use:   "antislop",
	Short: "Quality-only slop & over-engineering review (code or plan)",
	RunE:  commandAntislopAction,
}

func init() {
	commandAntislopCmd.Flags().String("workdir", ".", "working directory")
	commandAntislopCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	commandAntislopCmd.Flags().StringSliceP("model", "m", defaultAntislopModels, "antislop model(s): astra, fable (comma-separated)")
	commandAntislopCmd.Flags().String("effort", "", "override reasoning effort for every selected model: low, medium, high, ultra")
	commandCmd.AddCommand(commandAntislopCmd)
}

// buildAntislopCodePrompt assembles the code-mode prompt and the scope strings.
// target is recorded as the sessions' review scope; display goes on the output
// Scope line.
func buildAntislopCodePrompt(scope string, autoScope bool, workdir string) (prompt, target, display string) {
	if !autoScope {
		return strings.ReplaceAll(config.AntislopCodePrompt, "{SCOPE}", scope), scope, scope
	}
	preamble, files := buildDiffPreamble(workdir)
	if files == "" {
		log.Debug().Msg("git scope: no changes detected, falling back to full project")
		const full = "the entire project"
		return strings.ReplaceAll(config.AntislopCodePrompt, "{SCOPE}", full), full, full
	}
	log.Info().Str("files", files).Msg("git scope: auto-detected changed files")
	prompt = preamble + strings.ReplaceAll(config.AntislopCodePrompt, "{SCOPE}", "the changed files listed above")
	return prompt, files, "changed files (git auto-detect)"
}

func commandAntislopAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	rawModels, _ := cmd.Flags().GetStringSlice("model")
	flagEffort, _ := cmd.Flags().GetString("effort")
	effortSet := cmd.Flags().Changed("effort")

	if effortSet && !config.IsValidEffort(flagEffort) {
		err := fmt.Errorf("invalid effort %q, must be one of: %v", flagEffort, config.ValidEfforts)
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	// If stdin is a terminal, show usage instead of hanging. Guard against a nil
	// stat (stdin closed/invalid) so we don't panic dereferencing it.
	if stat, statErr := os.Stdin.Stat(); statErr == nil && (stat.Mode()&os.ModeCharDevice) != 0 {
		_, _ = fmt.Fprintln(os.Stdout, antislopUsage)
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
		_, _ = fmt.Fprintln(os.Stdout, antislopUsage)
		return nil
	}
	if err := rejectUnresolvedMR(string(raw)); err != nil {
		return err
	}

	if cmd.Flags().Changed("model") && len(parsed.Models) > 0 {
		err := fmt.Errorf("model selection was provided both as --model command flags and in arguments; use one form")
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}
	modelSelectors := rawModels
	if len(parsed.Models) > 0 {
		modelSelectors = parsed.Models
	}
	clis, err := parsePlanModels(modelSelectors)
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	effort, err := mergePlanEffort(flagEffort, effortSet, parsed.Effort)
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	prompt, target, display := buildAntislopCodePrompt(parsed.ReviewScope, parsed.AutoScope, workdir)

	// Cancel the queue wait / child processes on SIGINT/SIGTERM.
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	groupID := uuid.New().String()

	result, err := review.RunDocReview(ctx, session.ModeAntislop, prompt, target, effort, config.DefaultAntislopEffort, workdir, groupID, noQueue, clis)
	if err != nil {
		return err
	}

	out := review.FormatAntislopResult(result, display)
	if _, err := io.WriteString(os.Stdout, out); err != nil {
		return fmt.Errorf("write stdout: %w", err)
	}
	return nil
}
