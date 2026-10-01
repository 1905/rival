package review

import (
	"context"
	"errors"
	"fmt"
	"os"
	"strings"
	"sync"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/executor"
	"github.com/1905/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// buildPlanPrompt renders the plan-review prompt for a plan file at absPath.
func buildPlanPrompt(absPath string) string {
	return strings.ReplaceAll(config.PlanReviewPrompt, "{FILE}", absPath)
}

// RunTimeoutReason turns a failed run into a session-failure reason. When the
// run's context hit the RIVAL_RUN_TIMEOUT deadline it reports the timeout, so
// a hung provider is distinguishable from a normal failure; otherwise it
// returns fallback, which is the provider's own error text.
//
// label names the model in the message. Pass an empty label when the caller
// has no model name to report.
func RunTimeoutReason(ctx context.Context, label, fallback string) string {
	if !errors.Is(ctx.Err(), context.DeadlineExceeded) {
		return fallback
	}
	if label == "" {
		return fmt.Sprintf("run timeout after %s (RIVAL_RUN_TIMEOUT) — model did not finish", config.RunTimeout())
	}
	return fmt.Sprintf("%s run timeout after %s (RIVAL_RUN_TIMEOUT) — model did not finish", label, config.RunTimeout())
}

// RunFailureReason reports why a run that exited 0 still produced no review:
// an empty log, or a provider quota/rate-limit message when nothing parsed.
// It returns "" otherwise; output that merely fails to parse is not a run
// failure (it prints as unparsed). Quota is checked only when nothing parsed:
// the signatures are plain substrings, so a review of code that mentions them
// must not fail. label prefixes the reason; an empty label returns it bare.
func RunFailureReason(label, raw string, parsed bool) string {
	var reason string
	switch {
	case strings.TrimSpace(raw) == "":
		reason = "produced no output (empty result); likely an auth/session failure"
	case !parsed && executor.IsQuotaExhausted(raw):
		reason = "hit provider quota/rate limit (429)"
	default:
		return ""
	}
	if label == "" {
		return reason
	}
	return label + " " + reason
}

// planFailureReason keeps user-facing failures model-specific even though the
// implementation uses local adapter binaries underneath.
func planFailureReason(cli, reason string) string {
	return config.PublicRuntimeError(cli, planModelForCLI(cli), reason)
}

// PlanRunResult holds the outcome of a plan review across one or more CLIs.
// Results carries the successful reviews (Parsed may be nil if a CLI's output
// failed to parse — its Raw is retained); Skipped records CLIs that were
// unavailable, failed, or hit quota so the caller can surface why.
type PlanRunResult struct {
	Results []PlanCLIResult
	Skipped []SkippedCLI
}

// planCLIRun is the raw outcome of running one CLI's plan review, before quota
// detection and JSON parsing. assemblePlanResults turns a batch of these (plus
// the pre-run skipped list) into the final PlanRunResult — keeping that logic
// pure and unit-testable without shelling real CLIs.
type planCLIRun struct {
	CLI      string
	Model    string
	Raw      string
	ExitCode int
	Err      error
	// Reason is a human-readable failure reason set on the error path (e.g. a
	// RIVAL_RUN_TIMEOUT message or the provider's own error), so a skipped CLI
	// reports the real failure mode instead of a bare "exited with code -1".
	Reason string
}

// planExecutor wraps the preflight + run steps behind func fields so tests can
// inject fakes. The zero value is unusable; use defaultPlanExecutor().
type planExecutor struct {
	preflight func(cli string) error
	// run executes one CLI's plan review on sess and returns the raw log output.
	run func(ctx context.Context, sess *session.Session, cli, prompt, effort, workdir string) (raw string, exitCode int, err error)
}

func defaultPlanExecutor() planExecutor {
	return planExecutor{
		preflight: func(cli string) error {
			switch cli {
			case "codex":
				return executor.CodexPreflightFor(planModelForCLI(cli))
			case "claude":
				return executor.ClaudePreflight()
			default:
				return fmt.Errorf("unsupported plan cli: %s", cli)
			}
		},
		run: func(ctx context.Context, sess *session.Session, cli, prompt, effort, workdir string) (string, int, error) {
			var result *executor.Result
			var err error
			switch cli {
			case "codex":
				result, err = executor.RunCodexModel(ctx, sess, prompt, effort, workdir, planModelForCLI(cli), nil)
			case "claude":
				result, err = executor.RunClaude(ctx, sess, prompt, effort, workdir, nil)
			default:
				return "", 1, fmt.Errorf("unsupported plan cli: %s", cli)
			}
			if err != nil {
				return "", 1, err
			}
			logData, readErr := os.ReadFile(sess.LogFile)
			if readErr != nil {
				return "", result.ExitCode, fmt.Errorf("read log: %w", readErr)
			}
			return string(logData), result.ExitCode, nil
		},
	}
}

// planModelForCLI returns the model id recorded for a plan CLI's session.
func planModelForCLI(cli string) string {
	switch cli {
	case "codex":
		return config.CodexModel
	case "claude":
		return config.ClaudeModel
	default:
		return cli
	}
}

// RunPlanReview reviews the plan/spec file at absPath with each CLI in clis,
// concurrently, under a single queue ticket. effort is an optional invocation
// override; when empty, each model resolves its own configured/default effort.
// There is no judge — each CLI's structured 1-10 rating + findings
// are returned independently. A CLI that is unavailable, fails, or hits quota
// is recorded in Skipped rather than aborting the run; only when every CLI is
// unusable does this return an error.
func RunPlanReview(ctx context.Context, absPath, effort, workdir, groupID string, noQueue bool, clis []string) (*PlanRunResult, error) {
	return runPlanReview(ctx, defaultPlanExecutor(), absPath, effort, workdir, groupID, noQueue, clis)
}

// RunDocReview runs a plan-shaped review (summary + 1-10 rating + findings)
// with a caller-built prompt. target is recorded as the sessions' review scope.
// fallbackEffort is the surface's default when neither the invocation nor
// ~/.rival/config.yaml names an effort; empty keeps the plan-review defaults
// (DefaultPlanEffort). Antislop resolves its own high default.
// mode is the session mode and the queue ticket label ("plan" or "antislop").
// Queue behavior does not depend on it: the concurrency limit is global.
func RunDocReview(ctx context.Context, mode, prompt, target, effort, fallbackEffort, workdir, groupID string, noQueue bool, clis []string) (*PlanRunResult, error) {
	return runDocReview(ctx, defaultPlanExecutor(), mode, prompt, target, effort, fallbackEffort, workdir, groupID, noQueue, clis)
}

func runPlanReview(ctx context.Context, ex planExecutor, absPath, effort, workdir, groupID string, noQueue bool, clis []string) (*PlanRunResult, error) {
	return runDocReview(ctx, ex, "plan", buildPlanPrompt(absPath), absPath, effort, "", workdir, groupID, noQueue, clis)
}

func runDocReview(ctx context.Context, ex planExecutor, mode, prompt, target, effort, fallbackEffort, workdir, groupID string, noQueue bool, clis []string) (*PlanRunResult, error) {
	if len(clis) == 0 {
		return nil, fmt.Errorf("no plan models requested")
	}

	// Preflight each CLI BEFORE enqueuing so a doomed run never occupies a slot.
	// Unavailable CLIs are skipped; sessions are created only for the survivors.
	type plan struct {
		cli  string
		sess *session.Session
	}
	var plans []plan
	var skipped []SkippedCLI
	var sessions []*session.Session

	// Fail any created session still "queued" (never started) when we return —
	// registered BEFORE the creation loop so a NewQueued failure mid-loop still
	// cleans up the sessions already created, not just the ones that started.
	defer func() {
		for _, s := range sessions {
			if s.Status == "queued" {
				_ = s.Fail(1, "interrupted")
			}
		}
	}()

	for _, cli := range clis {
		if err := ex.preflight(cli); err != nil {
			model := planModelForCLI(cli)
			reason := planFailureReason(cli, err.Error())
			log.Warn().Str("reviewer", config.EngineLabel(cli, model)).Str("reason", reason).Msg("plan reviewer unavailable")
			skipped = append(skipped, SkippedCLI{CLI: cli, Model: model, Reason: reason})
			continue
		}
		model := planModelForCLI(cli)
		modelFallback := fallbackEffort
		if modelFallback == "" {
			modelFallback = config.DefaultPlanEffort
			if cli == "codex" {
				modelFallback = "xhigh"
			}
		}
		var effectiveEffort string
		var err error
		if mode == session.ModeAntislop {
			effectiveEffort, err = config.ResolveAntislopEffort(model, effort)
		} else {
			effectiveEffort, err = config.ResolveEffort(model, effort, modelFallback)
		}
		if err != nil {
			return nil, fmt.Errorf("resolve %s plan effort: %w", config.EngineLabel(cli, model), err)
		}
		sess, err := session.NewQueued(cli, mode, model, effectiveEffort, workdir, prompt, target, groupID)
		if err != nil {
			return nil, fmt.Errorf("create %s plan session: %w", config.EngineLabel(cli, model), err)
		}
		if cli == "claude" {
			sess.Account = config.ClaudeSubscription()
		}
		plans = append(plans, plan{cli: cli, sess: sess})
		sessions = append(sessions, sess)
	}

	if len(plans) == 0 {
		return nil, fmt.Errorf("no plan models available (see skipped reasons): %s", formatSkipped(skipped))
	}

	// One queue ticket covers all plan sessions; all of them are the run set.
	release, err := WaitForGroupSlot(ctx, noQueue, sessions, sessions, workdir, groupID, mode)
	if err != nil {
		return nil, err
	}
	defer release()

	// Bound the run once a slot is held: a hung CLI must not keep the slot (and
	// the detached rival) alive forever. Single phase → mult 1.
	ctx, cancelRun := config.WithRunTimeout(ctx, 1)
	defer cancelRun()

	// Run every CLI concurrently.
	type indexedRun struct {
		index int
		run   planCLIRun
	}
	var wg sync.WaitGroup
	runs := make(chan indexedRun, len(plans))
	for i, p := range plans {
		wg.Add(1)
		go func(index int, pl plan) {
			defer wg.Done()
			runs <- indexedRun{index: index, run: runPlanCLI(ctx, ex, pl.sess, pl.cli, prompt, workdir, mode)}
		}(i, p)
	}
	wg.Wait()
	close(runs)

	batch := make([]planCLIRun, len(plans))
	for r := range runs {
		batch[r.index] = r.run
	}

	return assemblePlanResults(batch, skipped)
}

// runPlanCLI executes one CLI's plan review and finalizes its session, returning
// the raw outcome for assemblePlanResults to interpret.
func runPlanCLI(ctx context.Context, ex planExecutor, sess *session.Session, cli, prompt, workdir, mode string) planCLIRun {
	model := planModelForCLI(cli)

	defer func() {
		if sess.Status == "running" || sess.Status == "queued" {
			_ = sess.Fail(1, "interrupted")
		}
	}()

	log.Info().Str("session", sess.ID).Str("reviewer", config.EngineLabel(cli, model)).Msg("starting plan reviewer")

	raw, exitCode, err := ex.run(ctx, sess, cli, prompt, sess.Effort, workdir)

	// Keep this defensive restoration for injected/custom executors. The built-in
	// Claude executor preserves the session mode throughout the live run.
	sess.Mode = mode

	if err != nil {
		reason := RunTimeoutReason(ctx, config.EngineLabel(cli, model), planFailureReason(cli, err.Error()))
		_ = sess.Fail(1, reason)
		return planCLIRun{CLI: cli, Model: model, Err: err, Reason: reason, ExitCode: -1}
	}

	label := config.EngineLabel(cli, model)
	// Parse only the final answer: a plan payload printed earlier by a tool
	// (a file the model read) must not pass for the review.
	_, parseErr := ParsePlanOutput(FinalAnswer(raw))
	if exitCode != 0 {
		_ = sess.Fail(exitCode, fmt.Sprintf("%s exited with code %d", label, exitCode))
	} else if reason := RunFailureReason(label, raw, parseErr == nil); reason != "" {
		// Fail it so it is reported as skipped, not a "successful" plan review
		// that formats to an empty string while the command exits 0.
		_ = sess.Fail(1, reason)
	} else {
		_ = sess.Complete(exitCode, int64(len(raw)), 0)
	}

	return planCLIRun{CLI: cli, Model: model, Raw: raw, ExitCode: exitCode}
}

// assemblePlanResults turns raw CLI runs (plus the pre-run skipped list) into the
// final PlanRunResult. It is pure: given the same inputs it produces the same
// output with no I/O, so the full success/failure matrix is unit-testable without
// real CLIs. A CLI that errored, exited non-zero, or hit quota is moved to
// Skipped; a successful CLI whose output does not parse keeps its Raw with a nil
// Parsed so nothing is lost. If no CLI succeeds, it returns an error listing the
// skipped reasons.
func assemblePlanResults(batch []planCLIRun, skipped []SkippedCLI) (*PlanRunResult, error) {
	var results []PlanCLIResult
	for _, r := range batch {
		model := r.Model
		if model == "" {
			model = planModelForCLI(r.CLI)
		}
		switch {
		case r.Err != nil:
			reason := r.Reason
			if reason == "" {
				reason = planFailureReason(r.CLI, r.Err.Error())
			}
			skipped = append(skipped, SkippedCLI{CLI: r.CLI, Model: model, Reason: reason})
			continue
		case r.ExitCode != 0:
			skipped = append(skipped, SkippedCLI{CLI: r.CLI, Model: model, Reason: fmt.Sprintf("exited with code %d", r.ExitCode)})
			continue
		}

		parsed, parseErr := ParsePlanOutput(FinalAnswer(r.Raw))
		if reason := RunFailureReason("", r.Raw, parseErr == nil); reason != "" {
			// A single-CLI run must never format to an empty string with exit 0.
			skipped = append(skipped, SkippedCLI{CLI: r.CLI, Model: model, Reason: reason})
			continue
		}
		res := PlanCLIResult{CLI: r.CLI, Model: model, Raw: r.Raw}
		if parseErr != nil {
			log.Warn().Str("reviewer", config.EngineLabel(r.CLI, model)).Err(parseErr).Msg("failed to parse plan output, keeping raw")
		} else {
			res.Parsed = parsed
		}
		results = append(results, res)
	}

	if len(results) == 0 {
		return nil, fmt.Errorf("all plan reviewers failed or hit quota limits (see skipped reasons): %s", formatSkipped(skipped))
	}

	return &PlanRunResult{Results: results, Skipped: skipped}, nil
}
