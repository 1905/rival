package cmd

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/executor"
	"github.com/1F47E/rival/internal/parser"
	"github.com/1F47E/rival/internal/review"
	"github.com/1F47E/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// modelSpec describes one model's command and run surfaces. It carries only
// what genuinely differs per model. Anything a single model needs stays an
// explicit branch in the workflows below, keyed on commandName, rather than
// becoming a callback field nobody else sets.
type modelSpec struct {
	// commandName is the cobra command word: sol, fable, k3, or grok. For K3
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
	// run invokes the provider. review reports whether the run is a review, so
	// grok can apply its sandbox; out is the stdout mirror, nil in command mode.
	run func(ctx context.Context, sess *session.Session, prompt, effort, workdir string, review bool, out io.Writer) (*executor.Result, error)
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
	if s.commandName == config.FableLabel || s.commandName == config.AstraLabel {
		// Fable and Astra resolve their own configured defaults rather than
		// the shared review one: a non-empty fallback here short-circuits
		// builtinModelEffort and would silently override Astra's xhigh.
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
// provider has none. Only Fable distinguishes auth failures this way.
func (s modelSpec) authHint(logFile string) string {
	if s.commandName != config.FableLabel {
		return ""
	}
	return executor.ClaudeAuthHint(logFile)
}

// runModelCommand is the shared command-surface workflow: read args from
// stdin, run the provider, then print the log for the calling skill to
// capture. Every model's `rival command <name>` goes through it.
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
	if err := rejectUnresolvedMR(string(raw)); err != nil {
		return err
	}

	if parsed.IsReview && parsed.AutoScope {
		resolveGitScope(parsed, workdir)
	}

	effort, err := spec.resolveEffort(parsed.Effort)
	if err != nil {
		return err
	}
	if err := spec.preflight(workdir); err != nil {
		return err
	}

	mode := sessionMode(parsed.IsReview)
	sess, err := session.NewQueued(spec.cli, mode, spec.model, effort, workdir, parsed.Prompt, parsed.ReviewScope, "")
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
		Msgf("starting %s (command mode)", spec.commandName)

	// Cancel the queue wait and the child on SIGINT/SIGTERM so the deferred
	// Fail runs.
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	sessions := []*session.Session{sess}
	release, err := review.WaitForGroupSlot(ctx, noQueue, sessions, sessions, workdir, sess.GroupID, mode)
	if err != nil {
		return err
	}
	defer release()

	// Bound the run: a hung provider must not hold the slot forever. The clock
	// starts after slot promotion.
	runCtx, cancelRun := config.WithRunTimeout(ctx, 1)
	defer cancelRun()

	// No stdout mirror in command mode; the skill reads the final output.
	result, err := spec.run(runCtx, sess, parsed.Prompt, effort, workdir, parsed.IsReview, nil)
	if err != nil {
		failSession(sess, 1, review.RunTimeoutReason(runCtx, spec.label(), err.Error()))
		return err
	}

	exitMsg := fmt.Sprintf("%s exited with code %d", spec.label(), result.ExitCode)
	if result.ExitCode != 0 {
		failSession(sess, result.ExitCode, review.RunTimeoutReason(runCtx, spec.label(), exitMsg))
	} else if saveErr := sess.Complete(result.ExitCode, result.OutputBytes, result.OutputLines); saveErr != nil {
		log.Warn().Err(saveErr).Str("session", sess.ID).Msg("failed to save session completion")
	}

	logData, err := os.ReadFile(sess.LogFile)
	if err != nil {
		return fmt.Errorf("read log file: %w", err)
	}
	if _, err := io.WriteString(os.Stdout, config.PublicRuntimeLog(sess.CLI, sess.Model, string(logData))); err != nil {
		return fmt.Errorf("write stdout: %w", err)
	}

	if result.ExitCode != 0 {
		// The hint follows the log on stdout, so a skill capturing output sees
		// the failure before the explanation.
		if hint := spec.authHint(sess.LogFile); hint != "" {
			_, _ = fmt.Fprintln(os.Stdout, "\n"+hint)
		}
		return &ExitCodeError{Code: result.ExitCode, Err: fmt.Errorf("%s", exitMsg)}
	}
	return nil
}

// failSession records a failure and logs when the record cannot be saved.
func failSession(sess *session.Session, exitCode int, reason string) {
	if err := sess.Fail(exitCode, reason); err != nil {
		log.Warn().Err(err).Str("session", sess.ID).Msg("failed to save session failure")
	}
}
