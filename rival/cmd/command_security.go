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
	"github.com/1F47E/rival/internal/executor"
	"github.com/1F47E/rival/internal/review"
	"github.com/1F47E/rival/internal/session"
	"github.com/rs/zerolog/log"
	"github.com/spf13/cobra"
)

const securityUsage = `Usage:
  /rival-security — security review of the changed files (git auto-detect)
  /rival-security src/api/ — review a specific scope
  rival command security --which — show which model will run and why

The model comes from security.reviewer in ~/.rival/config.yaml: k3 (default)
or grok. Run --which to see the resolved model and whether its API key is
present. The run fails rather than falling back, because a security review
that silently skips is worse than one that refuses to start.`

var commandSecurityCmd = &cobra.Command{
	Use:   "security",
	Short: "Security review with the configured model",
	RunE:  commandSecurityAction,
}

func init() {
	commandSecurityCmd.Flags().String("workdir", ".", "working directory")
	commandSecurityCmd.Flags().Bool("no-queue", false, "bypass the review queue")
	commandSecurityCmd.Flags().Bool("which", false, "print the resolved model and exit")
	commandCmd.AddCommand(commandSecurityCmd)
}

// securityScopeAndPrompt builds the review prompt.
//
// It deliberately does NOT call resolveGitScope: that helper's last act is to
// overwrite the prompt with config.ReviewPrompt, the bug-hunter text, so a
// security command using it would run a bug hunt while reporting a security
// review. Empty stdin is this command's default invocation, so that mistake
// would have been the normal path.
func securityScopeAndPrompt(rawScope, workdir string) (prompt, scope string) {
	if scope = strings.TrimSpace(rawScope); scope != "" {
		return review.BuildReviewerPrompt(scope, config.PromptSecurity), scope
	}

	preamble, files := buildDiffPreamble(workdir)
	if files == "" {
		log.Debug().Msg("git scope: no changes detected, reviewing the whole project")
		const whole = "the entire project"
		return review.BuildReviewerPrompt(whole, config.PromptSecurity), whole
	}
	log.Info().Str("files", files).Msg("git scope: auto-detected changed files")
	return preamble + review.BuildReviewerPrompt("the changed files listed above", config.PromptSecurity),
		"changed files (git auto-detect)"
}

// printSecurityResolution reports which model will run, and whether it can.
// It exits non-zero when the model is unusable so a caller can check before
// launching a detached run.
func printSecurityResolution(entry config.SecurityModel, workdir string) error {
	configured := "unset (default)"
	if userConfigured := config.ConfiguredSecurityReviewer(); userConfigured != "" {
		configured = userConfigured
	}
	fmt.Printf("Security reviewer: %s (%s via %s)\n", entry.Name, entry.Model, entry.Provider)
	fmt.Printf("Config: security.reviewer = %s\n", configured)
	fmt.Printf("OpenCode selector: %s\n", entry.Selector)
	fmt.Printf("Reasoning variant: %s\n", entry.Variant)

	keySet := config.SecurityAPIKeyFrom(entry, workdir) != ""
	status := "MISSING"
	if keySet {
		status = "set"
	}
	fmt.Printf("%s: %s\n", entry.KeyEnv, status)

	// A present key does not make the model usable when the binary is absent,
	// so report the two conditions separately. The error travels only through
	// ExitCodeError: Execute prints it, and printing here as well would show
	// the same line twice.
	if err := executor.OpencodePreflightEntry(entry, workdir); err != nil {
		fmt.Printf("\nNot usable.\n")
		return &ExitCodeError{Code: 1, Err: err}
	}
	fmt.Printf("\nReady.\n")
	return nil
}

func commandSecurityAction(cmd *cobra.Command, args []string) error {
	workdir, _ := cmd.Flags().GetString("workdir")
	noQueue, _ := cmd.Flags().GetBool("no-queue")
	which, _ := cmd.Flags().GetBool("which")

	entry, err := config.ResolveSecurityModel()
	if err != nil {
		_, _ = fmt.Fprintln(os.Stderr, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	if which {
		return printSecurityResolution(entry, workdir)
	}

	// A terminal stdin means the command was run by hand with no piped scope.
	// Show usage rather than silently reviewing the whole project.
	if stat, statErr := os.Stdin.Stat(); statErr == nil && (stat.Mode()&os.ModeCharDevice) != 0 {
		_, _ = fmt.Fprintln(os.Stdout, securityUsage)
		return nil
	}

	var rawScope string
	if stat, statErr := os.Stdin.Stat(); statErr == nil && (stat.Mode()&os.ModeCharDevice) == 0 {
		raw, readErr := io.ReadAll(os.Stdin)
		if readErr != nil {
			return fmt.Errorf("read stdin: %w", readErr)
		}
		rawScope = string(raw)
	}

	if err := rejectUnresolvedMR(rawScope); err != nil {
		return err
	}
	if err := executor.OpencodePreflightEntry(entry, workdir); err != nil {
		hint := fmt.Errorf("%w\n\nsecurity.reviewer selects the model; accepted values: %s",
			err, strings.Join(config.SecurityReviewerNames(), ", "))
		_, _ = fmt.Fprintln(os.Stdout, hint.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}

	prompt, scope := securityScopeAndPrompt(rawScope, workdir)

	sess, err := session.NewQueued("opencode", session.ModeSecurity, entry.Model,
		entry.Variant, workdir, prompt, scope, "")
	if err != nil {
		return fmt.Errorf("create session: %w", err)
	}
	defer func() {
		if sess.Status == "running" || sess.Status == "queued" {
			_ = sess.Fail(1, "interrupted")
		}
	}()

	// `rival wait --log` only discovers a session from a line whose message
	// starts with "starting ". Any other wording leaves the watcher with no
	// session, and it then reports a healthy run as a crash.
	log.Info().Str("session", sess.ID).Str("model", entry.Label).
		Msg("starting security reviewer")

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	sessions := []*session.Session{sess}
	release, err := review.WaitForGroupSlot(ctx, noQueue, sessions, sessions, workdir, "", session.ModeSecurity)
	if err != nil {
		return err
	}
	defer release()

	runCtx, cancelRun := config.WithRunTimeout(ctx, 1)
	defer cancelRun()

	result, err := executor.RunOpencodeEntry(runCtx, sess, prompt, entry.Variant, workdir, entry,
		executor.OpencodeRunOpts{}, nil)
	if err != nil {
		failSession(sess, 1, review.RunTimeoutReason(runCtx, entry.Label, err.Error()))
		return err
	}

	logData, readErr := os.ReadFile(sess.LogFile)
	if readErr != nil {
		failSession(sess, 1, fmt.Sprintf("read log: %v", readErr))
		return fmt.Errorf("read log file: %w", readErr)
	}
	raw := string(logData)

	if result.ExitCode != 0 {
		exitMsg := fmt.Sprintf("%s exited with code %d", entry.Label, result.ExitCode)
		failSession(sess, result.ExitCode, review.RunTimeoutReason(runCtx, entry.Label, exitMsg))
		_, _ = io.WriteString(os.Stdout, config.PublicRuntimeLog("opencode", entry.Model, raw))
		return &ExitCodeError{Code: result.ExitCode, Err: fmt.Errorf("%s", exitMsg)}
	}

	parsed, parseErr := review.ParseReviewerOutput(raw)
	if parseErr != nil {
		log.Warn().Err(parseErr).Msg("security output did not parse")
	}
	out := review.FormatSecurityResult(parsed, raw, "opencode", entry.Model, scope)
	if _, err := io.WriteString(os.Stdout, out); err != nil {
		return fmt.Errorf("write stdout: %w", err)
	}

	// A security gate must not exit 0 on output it cannot trust. Non-empty
	// output is not evidence a review happened: it can be an echoed prompt,
	// truncated JSON, or an unrecognized provider error.
	if validErr := review.ValidateSecurityResult(parsed, raw); validErr != nil {
		failSession(sess, 1, fmt.Sprintf("unusable security output: %v", validErr))
		return &ExitCodeError{Code: 1, Err: fmt.Errorf("security review produced no usable findings: %w", validErr)}
	}

	if saveErr := sess.Complete(result.ExitCode, result.OutputBytes, result.OutputLines); saveErr != nil {
		// Complete mutates the in-memory status before saving, so the deferred
		// cleanup no longer sees a running session and skips its own write.
		// Exiting 0 here would leave the stored session running forever, and a
		// detached `rival wait` reports that as a crash.
		log.Warn().Err(saveErr).Str("session", sess.ID).Msg("failed to save session completion")
		failSession(sess, 1, fmt.Sprintf("could not persist completion: %v", saveErr))
		return &ExitCodeError{Code: 1, Err: fmt.Errorf("save session completion: %w", saveErr)}
	}
	return nil
}
