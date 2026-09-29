package cmd

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/review"
	"github.com/1905/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// runModelRun is the shared run-surface workflow. It differs from the command
// surface in ways that are deliberate, not incidental: the prompt comes from
// flags rather than parsed stdin args, output mirrors to stdout as it
// arrives, only a successful review reads the log back (to print the
// formatted findings after the mirror), and a nonzero exit returns
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
	switch {
	case opts.isReview:
		// Built below, once an MR scope has its checkout.
	case opts.promptStdin:
		data, err := io.ReadAll(os.Stdin)
		if err != nil {
			return fmt.Errorf("read stdin: %w", err)
		}
		prompt = string(data)
		if prompt == "" {
			return fmt.Errorf("empty prompt")
		}
		if err := rejectUnresolvedMR(prompt); err != nil {
			return err
		}
	default:
		return fmt.Errorf("provide --prompt-stdin or --review")
	}
	// Preflight in the caller's workdir: that is where credentials live, even
	// when an MR review later runs in a temporary checkout.
	if err := spec.preflight(opts.workdir); err != nil {
		return err
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	mode, scope, runWorkdir := "raw", opts.reviewScope, opts.workdir
	if opts.isReview {
		mode = "review"
		var closeReview func()
		var prepErr error
		prompt, scope, runWorkdir, closeReview, prepErr = prepareReview(ctx, scope, scope == "", opts.workdir)
		if prepErr != nil {
			return prepErr
		}
		defer closeReview()
	}

	sess, err := session.NewQueued(spec.cli, mode, spec.model, effort, runWorkdir, prompt, scope, "")
	if err != nil {
		return fmt.Errorf("create session: %w", err)
	}
	if spec.commandName == config.ClaudeLabel {
		sess.Account = config.ClaudeSubscription()
	}

	defer func() {
		if sess.Status == "running" || sess.Status == "queued" {
			_ = sess.Fail(1, "interrupted")
		}
	}()

	log.Info().Str("session", sess.ID).Str("effort", effort).Str("mode", mode).
		Msgf("starting %s", spec.commandName)

	sessions := []*session.Session{sess}
	release, err := review.WaitForGroupSlot(ctx, opts.noQueue, sessions, sessions, runWorkdir, sess.GroupID, mode)
	if err != nil {
		return err
	}
	defer release()

	runCtx, cancelRun := config.WithRunTimeout(ctx, 1)
	defer cancelRun()

	// The run surface is terminal-facing, so output mirrors to stdout live.
	result, err := spec.run(runCtx, sess, prompt, effort, runWorkdir, opts.workdir, opts.isReview, os.Stdout)
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

	if !opts.isReview {
		completeSession(sess, result)
		return nil
	}
	logData, err := os.ReadFile(sess.LogFile)
	if err != nil {
		failSession(sess, 1, "read log file: "+err.Error())
		return fmt.Errorf("read log file: %w", err)
	}
	// A zero exit is not a review: quota errors and empty output also exit 0.
	out, reason := finishReview(spec, sess, string(logData), scope, sess.LogFile)
	if reason != "" {
		_, _ = fmt.Fprintln(os.Stderr, reason)
		return &ExitCodeError{Code: 1, Err: fmt.Errorf("%s", reason)}
	}
	completeSession(sess, result)
	// The live mirror already showed the transcript; the formatted review
	// follows it.
	if _, err := io.WriteString(os.Stdout, "\n"+out); err != nil {
		return fmt.Errorf("write stdout: %w", err)
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
