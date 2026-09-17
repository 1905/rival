package review

import (
	"context"
	"fmt"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/executor"
	"github.com/1F47E/rival/internal/session"
)

// The megareview pipeline dispatches on a reviewer's CLI in three places:
// preflight, the reviewer run, and the consilium judge run. Each lookup lives
// here as a table returning (fn, ok) so a CLI that is wired into one switch but
// missed in another is a test failure rather than a runtime "unsupported cli".

// reviewRunner executes one reviewer or judge turn on a pre-created session.
type reviewRunner func(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string) (*executor.Result, error)

// grokReviewRun is the model-taking grok entry point the adapter must use; the
// default-model wrapper would silently discard the session's model.
var grokReviewRun = executor.RunGrokModel

// preflightFor returns the availability check for a reviewer CLI. Model and
// workdir are passed because OpenCode resolves credentials per model and per
// project; the other CLIs ignore them.
func preflightFor(cli string) (func(model, workdir string) error, bool) {
	switch cli {
	case "codex":
		return func(model, _ string) error { return executor.CodexPreflightFor(model) }, true
	case "opencode":
		return executor.OpencodePreflightModel, true
	case config.GrokLabel:
		return func(string, string) error { return executor.GrokPreflight() }, true
	default:
		return nil, false
	}
}

// reviewerRunnerFor returns the adapter that runs one reviewer's pass.
func reviewerRunnerFor(cli string) (reviewRunner, bool) {
	switch cli {
	case "codex":
		return func(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string) (*executor.Result, error) {
			return executor.RunCodexModel(ctx, sess, prompt, effort, workdir, model, nil)
		}, true
	case "opencode":
		return func(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string) (*executor.Result, error) {
			opts := executor.OpencodeRunOpts{APIKey: config.KimiAPIKeyFrom(credentialWorkdir(ctx, workdir))}
			return executor.RunOpencodeWith(ctx, sess, prompt, effort, workdir, model, opts, nil)
		}, true
	case config.GrokLabel:
		return func(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string) (*executor.Result, error) {
			// review=true puts grok in its mechanically read-only sandbox.
			// The model comes from the session so the judge honors the same
			// "session carries the concrete model" contract as the other CLIs;
			// RunGrokModel falls back to the default when it is empty.
			return grokReviewRun(ctx, sess, prompt, effort, workdir, model, true, nil)
		}, true
	default:
		return nil, false
	}
}

// judgeRunnerFor returns the adapter that runs the consilium judge. The judge
// runs the same read-only review shape as a reviewer.
func judgeRunnerFor(cli string) (reviewRunner, bool) {
	return reviewerRunnerFor(cli)
}

// grokReviewEffort clamps Rival's effort ladder onto grok's low/medium/high.
func grokReviewEffort(effort string) (string, error) {
	return executor.GrokEffort(effort)
}

// reviewerEffortFor resolves the effort a reviewer session records. For grok
// the recorded value is the clamped one, so the session never advertises an
// effort the provider was not actually sent.
func reviewerEffortFor(cli, model, override string) (string, error) {
	fallback := config.DefaultReviewEffort
	if model == config.AstraModel {
		fallback = "xhigh"
	}
	effort, err := config.ResolveEffort(model, override, fallback)
	if err != nil {
		return "", err
	}
	if cli == config.GrokLabel {
		clamped, clampErr := grokReviewEffort(effort)
		if clampErr != nil {
			return "", fmt.Errorf("%s reviewer: %w", config.GrokLabel, clampErr)
		}
		return clamped, nil
	}
	return effort, nil
}
