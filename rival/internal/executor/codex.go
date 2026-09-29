package executor

import (
	"context"
	"fmt"
	"io"
	"os/exec"
	"strings"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
)

// CodexPreflightFor checks that codex is installed and authenticated, naming
// the given model in its errors.
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

// RunCodexModel executes a prompt with one explicit model. Review pipelines use
// this entry point so the model recorded in the session is also the model sent
// to the runtime. Codex is the only model it runs.
func RunCodexModel(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string, mirror io.Writer) (*Result, error) {
	if model != config.CodexModel {
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
	// The codex runtime takes ultra as its own reasoning level (distinct from
	// xhigh), so preserve the requested value instead of normalizing it.
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
