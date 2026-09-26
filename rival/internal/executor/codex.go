package executor

import (
	"context"
	"fmt"
	"io"
	"os/exec"
	"strings"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
)

// CodexPreflight checks that codex is installed and authenticated, reporting
// failures under the default Sol label. Callers running another model on this
// runtime should use CodexPreflightFor so the diagnostic names their model.
func CodexPreflight() error {
	return CodexPreflightFor(config.GPT56SolModel)
}

// CodexPreflightFor is CodexPreflight with the model named in its errors.
// Sol and Codex share this runtime, so a hardcoded label would tell an Codex
// user that Sol is broken.
func CodexPreflightFor(model string) error {
	label := config.EngineLabel("codex", model)
	if _, err := exec.LookPath("codex"); err != nil {
		return fmt.Errorf("%s runtime is not installed", label)
	}

	cmd := exec.Command("codex", "login", "status")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("%s authentication is unavailable\n%s", label, string(out))
	}
	return nil
}

// RunCodex executes a prompt with the default GPT model. It remains as a
// compatibility wrapper for standalone callers.
func RunCodex(ctx context.Context, sess *session.Session, prompt, effort, workdir string, mirror io.Writer) (*Result, error) {
	return RunCodexModel(ctx, sess, prompt, effort, workdir, config.GPT56SolModel, mirror)
}

// RunCodexModel executes a prompt with one explicit model. Review pipelines use
// this entry point so the model recorded in the session is also the model sent
// to the runtime.
func RunCodexModel(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string, mirror io.Writer) (*Result, error) {
	if model == "" {
		model = config.GPT56SolModel
	}
	if model != config.GPT56SolModel && model != config.CodexModel {
		return nil, fmt.Errorf("unsupported codex model %q", model)
	}
	args := codexRunArgs(model, effort, workdir)

	fullPrompt := config.SystemPrompt + "\n\n" + config.BuildWorkdirPreamble(workdir) + "\n" + prompt
	result, err := RunSubprocess(ctx, sess, "codex", args, nil, fullPrompt, mirror)
	if err != nil {
		label := config.EngineLabel("codex", model)
		message := strings.NewReplacer("Codex", label, "codex", label, model, label).Replace(err.Error())
		return nil, fmt.Errorf("%s runtime: %s", label, message)
	}
	return result, nil
}

func codexRunArgs(model, effort, workdir string) []string {
	// gpt-5.6-sol exposes ultra as a native runtime effort (distinct from max),
	// so preserve the requested value instead of normalizing it.
	return []string{
		"exec",
		"-C", workdir,
		"-m", model,
		"-c", fmt.Sprintf("model_reasoning_effort=%s", effort),
		"--sandbox", "read-only",
		"--ephemeral",
		"--skip-git-repo-check",
		"--color", "never",
		"-",
	}
}
