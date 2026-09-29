package cmd

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/executor"
	"github.com/1905/rival/internal/parser"
	"github.com/1905/rival/internal/review"
	"github.com/1905/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// modelSpec describes one model's command and run surfaces. It carries only
// what genuinely differs per model. Anything a single model needs stays an
// explicit branch in the workflows below, keyed on commandName, rather than
// becoming a callback field nobody else sets.
type modelSpec struct {
	// commandName is the cobra command word: codex, claude, k3, or grok. For K3
	// this is NOT the display label, which is kimi-k3.
	commandName string
	// cli is the adapter recorded on the session: codex, claude, opencode, or
	// the grok label.
	cli string
	// model is the concrete model id. The display and error label is always
	// config.EngineLabel(cli, model).
	model string
	usage string
	parse func(string) (*parser.ParseResult, error)
	// preflight verifies the runtime is usable. Only K3 needs the workdir.
	preflight func(workdir string) error
	// run invokes the provider. workdir is where it runs; credWorkdir is the
	// caller's project, where a .env credential is looked up (they differ only
	// for a GitLab MR review in a temporary checkout). review reports whether
	// the run is a review, so grok can apply its sandbox; out is the stdout
	// mirror, nil in command mode.
	run func(ctx context.Context, sess *session.Session, prompt, effort, workdir, credWorkdir string, review bool, out io.Writer) (*executor.Result, error)
}

// label is the public name used in every message and log field.
func (s modelSpec) label() string {
	return config.EngineLabel(s.cli, s.model)
}

// resolveEffort applies the model's effort rules. K3 is pinned to the only
// level its provider supports; grok clamps the shared ladder onto its own
// shorter menu.
func (s modelSpec) resolveEffort(requested string) (string, error) {
	if s.commandName == config.K3CommandName {
		return "max", nil
	}
	fallback := config.DefaultReviewEffort
	if s.commandName == config.ClaudeLabel || s.commandName == config.CodexLabel {
		// Claude and Codex resolve their own configured defaults rather than
		// the shared review one: a non-empty fallback here short-circuits
		// builtinModelEffort and would silently override Codex's xhigh.
		fallback = ""
	}
	effort, err := config.ResolveEffort(s.model, requested, fallback)
	if err != nil {
		return "", err
	}
	if s.commandName == config.GrokLabel {
		return executor.GrokEffort(effort)
	}
	return effort, nil
}

// sessionMode names the run for the dashboards.
func sessionMode(isReview bool) string {
	if isReview {
		return "review"
	}
	return "raw"
}

// authHint returns a provider-specific hint for a failed run, or "" when the
// provider has none. Only Claude distinguishes auth failures this way.
func (s modelSpec) authHint(logFile string) string {
	if s.commandName != config.ClaudeLabel {
		return ""
	}
	return executor.ClaudeAuthHint(logFile)
}

// runModelCommand is the shared command-surface workflow: read args from
// stdin, run the provider, then print the result for the calling skill to
// capture. A successful review prints the formatted findings and the log
// path; a raw prompt, or any failed run, prints the log. Every model's
// `rival command <name>` goes through it.
func runModelCommand(spec modelSpec, workdir string, noQueue bool) error {
	// A terminal stdin means no piped args, so show usage instead of hanging.
	if stat, statErr := os.Stdin.Stat(); statErr == nil && (stat.Mode()&os.ModeCharDevice) != 0 {
		_, _ = fmt.Fprintln(os.Stdout, spec.usage)
		return nil
	}

	raw, err := io.ReadAll(os.Stdin)
	if err != nil {
		return fmt.Errorf("read stdin: %w", err)
	}

	parsed, err := spec.parse(string(raw))
	if err != nil {
		_, _ = fmt.Fprintln(os.Stdout, err.Error())
		return &ExitCodeError{Code: 1, Err: err}
	}
	if parsed.IsEmpty {
		_, _ = fmt.Fprintln(os.Stdout, spec.usage)
		return nil
	}
	// A review resolves an MR scope into a pinned checkout below; a raw
	// prompt cannot, so it is rejected before anything starts.
	if !parsed.IsReview {
		if err := rejectUnresolvedMR(string(raw)); err != nil {
			return err
		}
	}

	effort, err := spec.resolveEffort(parsed.Effort)
	if err != nil {
		return err
	}
	// Preflight in the caller's workdir: that is where credentials live, even
	// when an MR review later runs in a temporary checkout.
	if err := spec.preflight(workdir); err != nil {
		return err
	}

	// Cancel the MR resolve, the queue wait and the child on SIGINT/SIGTERM so
	// the deferred cleanup runs.
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	prompt, scope, runWorkdir := parsed.Prompt, parsed.ReviewScope, workdir
	if parsed.IsReview {
		var closeReview func()
		var prepErr error
		prompt, scope, runWorkdir, closeReview, prepErr = prepareReview(ctx, scope, parsed.AutoScope, workdir)
		if prepErr != nil {
			return prepErr
		}
		defer closeReview()
	}

	mode := sessionMode(parsed.IsReview)
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
		Msgf("starting %s (command mode)", spec.commandName)

	sessions := []*session.Session{sess}
	release, err := review.WaitForGroupSlot(ctx, noQueue, sessions, sessions, runWorkdir, sess.GroupID, mode)
	if err != nil {
		return err
	}
	defer release()

	// Bound the run: a hung provider must not hold the slot forever. The clock
	// starts after slot promotion.
	runCtx, cancelRun := config.WithRunTimeout(ctx, 1)
	defer cancelRun()

	// No stdout mirror in command mode; the skill reads the final output.
	result, err := spec.run(runCtx, sess, prompt, effort, runWorkdir, workdir, parsed.IsReview, nil)
	if err != nil {
		failSession(sess, 1, review.RunTimeoutReason(runCtx, spec.label(), err.Error()))
		return err
	}

	exitCode := result.ExitCode
	exitMsg := fmt.Sprintf("%s exited with code %d", spec.label(), exitCode)
	logData, readErr := os.ReadFile(sess.LogFile)
	out := config.PublicRuntimeLog(sess.CLI, sess.Model, string(logData))
	switch {
	case exitCode != 0:
		failSession(sess, exitCode, review.RunTimeoutReason(runCtx, spec.label(), exitMsg))
	case parsed.IsReview && readErr == nil:
		// A zero exit is not a review: quota errors and empty output also exit 0.
		formatted, reason := finishReview(spec, sess, string(logData), scope, sess.LogFile)
		if reason != "" {
			exitCode, exitMsg = 1, reason
		} else {
			completeSession(sess, result)
			out = formatted
		}
	default:
		completeSession(sess, result)
	}
	if readErr != nil {
		return fmt.Errorf("read log file: %w", readErr)
	}
	if _, err := io.WriteString(os.Stdout, out); err != nil {
		return fmt.Errorf("write stdout: %w", err)
	}

	if exitCode != 0 {
		// The hint follows the log on stdout, so a skill capturing output sees
		// the failure before the explanation.
		if hint := spec.authHint(sess.LogFile); hint != "" {
			_, _ = fmt.Fprintln(os.Stdout, "\n"+hint)
		}
		return &ExitCodeError{Code: exitCode, Err: fmt.Errorf("%s", exitMsg)}
	}
	return nil
}

// failSession records a failure and logs when the record cannot be saved.
func failSession(sess *session.Session, exitCode int, reason string) {
	if err := sess.Fail(exitCode, reason); err != nil {
		log.Warn().Err(err).Str("session", sess.ID).Msg("failed to save session failure")
	}
}

// prepareReview resolves a review's scope and builds its bug-hunter prompt.
// A GitLab MR scope is pinned to a snapshot checkout, which closeFn removes;
// call it after the run on every path. displayScope is what the session
// records and the output shows: the MR URL, the auto-detected file list, or
// the scope as given.
func prepareReview(ctx context.Context, scope string, autoScope bool, workdir string) (prompt, displayScope, runWorkdir string, closeFn func(), err error) {
	target, err := prepareReviewTarget(ctx, scope, workdir)
	if err != nil {
		return "", "", "", nil, err
	}
	// Bug-hunter records and shows the detected file list, not the display
	// placeholder the other commands use.
	prompt, recorded, _ := buildReviewPrompt(lensPrompt(config.PromptBugHunter), target.scope, autoScope, target.workdir)
	displayScope = target.display
	if autoScope {
		displayScope = recorded
	}
	return prompt, displayScope, target.workdir, target.close, nil
}

// finishReview turns a zero-exit review log into the formatted review. When
// the run produced no review (empty log, or only a quota error) it records the
// failure on sess and returns the reason instead; output that merely does not
// parse still formats, as UNPARSED.
func finishReview(spec modelSpec, sess *session.Session, raw, scope, logPath string) (out, failReason string) {
	parsed, err := review.ParseReviewerOutput(review.FinalAnswer(raw))
	if reason := review.RunFailureReason(spec.label(), raw, err == nil); reason != "" {
		failSession(sess, 1, reason)
		return "", reason
	}
	if err != nil {
		log.Warn().Err(err).Msg("review output did not parse")
	}
	return review.FormatReviewResult(parsed, raw, spec.cli, spec.model, scope, logPath), ""
}

// completeSession records a successful run and logs when the record cannot be
// saved.
func completeSession(sess *session.Session, result *executor.Result) {
	if err := sess.Complete(result.ExitCode, result.OutputBytes, result.OutputLines); err != nil {
		log.Warn().Err(err).Str("session", sess.ID).Msg("failed to save session completion")
	}
}
