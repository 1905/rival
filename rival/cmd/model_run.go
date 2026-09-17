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
	"github.com/1F47E/rival/internal/review"
	"github.com/1F47E/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// runModelRun is the shared run-surface workflow. It differs from the command
// surface in ways that are deliberate, not incidental: the prompt comes from
// flags rather than parsed stdin args, output mirrors to stdout as it
// arrives, there is no log readback afterwards, and a nonzero exit returns
// immediately instead of falling through.
func runModelRun(spec modelSpec, opts runOptions) error {
	// Effort first. It is pure validation, so a bad value must fail fast
	// rather than hide behind an auth error or block on --prompt-stdin.
	effort, err := spec.resolveEffort(opts.effort)
	if err != nil {
		return err
	}

	// --review wins over --prompt-stdin when both are given.
	var prompt string
	mode := "raw"
	switch {
	case opts.isReview:
		mode = "review"
		scope := opts.reviewScope
		if scope == "" {
			scope = "the entire project"
		}
		prompt = strings.ReplaceAll(config.ReviewPrompt, "{SCOPE}", scope)
	case opts.promptStdin:
		data, err := io.ReadAll(os.Stdin)
		if err != nil {
			return fmt.Errorf("read stdin: %w", err)
		}
		prompt = string(data)
	default:
		return fmt.Errorf("provide --prompt-stdin or --review")
	}
	if prompt == "" {
		return fmt.Errorf("empty prompt")
	}
	if err := rejectUnresolvedMR(prompt); err != nil {
		return err
	}
	if err := spec.preflight(opts.workdir); err != nil {
		return err
	}

	sess, err := session.NewQueued(spec.cli, mode, spec.model, effort, opts.workdir, prompt, opts.reviewScope, "")
	if err != nil {
		return fmt.Errorf("create session: %w", err)
	}
	if spec.commandName == config.FableLabel {
		sess.Account = config.ClaudeSubscription()
	}

	defer func() {
		if sess.Status == "running" || sess.Status == "queued" {
			_ = sess.Fail(1, "interrupted")
		}
	}()

	log.Info().Str("session", sess.ID).Str("effort", effort).Str("mode", mode).
		Msgf("starting %s", spec.commandName)

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	sessions := []*session.Session{sess}
	release, err := review.WaitForGroupSlot(ctx, opts.noQueue, sessions, sessions, opts.workdir, sess.GroupID, mode)
	if err != nil {
		return err
	}
	defer release()

	runCtx, cancelRun := config.WithRunTimeout(ctx, 1)
	defer cancelRun()

	// The run surface is terminal-facing, so output mirrors to stdout live.
	result, err := spec.run(runCtx, sess, prompt, effort, opts.workdir, opts.isReview, os.Stdout)
	if err != nil {
		failSession(sess, 1, review.RunTimeoutReason(runCtx, spec.label(), err.Error()))
		return err
	}

	if result.ExitCode != 0 {
		exitMsg := fmt.Sprintf("%s exited with code %d", spec.label(), result.ExitCode)
		// Record the provider's own exit code and reason before returning;
		// otherwise the deferred cleanup would overwrite both with a generic
		// interrupted failure.
		failSession(sess, result.ExitCode, review.RunTimeoutReason(runCtx, spec.label(), exitMsg))
		// The hint goes to stderr here, because stdout already carries the
		// mirrored provider output.
		if hint := spec.authHint(sess.LogFile); hint != "" {
			_, _ = fmt.Fprintln(os.Stderr, hint)
		}
		return &ExitCodeError{Code: result.ExitCode, Err: fmt.Errorf("%s", exitMsg)}
	}

	if saveErr := sess.Complete(result.ExitCode, result.OutputBytes, result.OutputLines); saveErr != nil {
		log.Warn().Err(saveErr).Str("session", sess.ID).Msg("failed to save session completion")
	}
	return nil
}

// runOptions carries the run surface's flag values.
type runOptions struct {
	workdir     string
	noQueue     bool
	effort      string
	reviewScope string
	isReview    bool
	promptStdin bool
}
