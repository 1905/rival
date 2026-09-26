package executor

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
	"github.com/rs/zerolog/log"
)

// GrokPreflight checks that grok is installed and authenticated. The auth file
// is resolved from the real home directory rather than $GROK_HOME: that prefix
// is blocked from child environments (see blockedEnvPrefixes), so honoring it
// here would check a location the run can never use.
func GrokPreflight() error {
	if _, err := exec.LookPath("grok"); err != nil {
		return fmt.Errorf("%s runtime is not installed", config.GrokLabel)
	}

	home, err := os.UserHomeDir()
	if err != nil {
		return fmt.Errorf("%s authentication is unavailable: %w; run `grok login`", config.GrokLabel, err)
	}
	authFile := filepath.Join(home, ".grok", "auth.json")
	if _, err := os.Stat(authFile); err != nil {
		return fmt.Errorf("%s authentication is unavailable (%s not found); run `grok login`", config.GrokLabel, authFile)
	}
	return nil
}

// grokSubprocess is the spawn step, indirected so tests can observe the argv
// RunGrokModel actually builds without executing the grok binary.
var grokSubprocess = RunSubprocess

// RunGrok executes a prompt with grok's default model. It is the entry point
// for the single-model surfaces (`rival command grok`, `rival run grok`), which
// have no per-run model choice.
func RunGrok(ctx context.Context, sess *session.Session, prompt, effort, workdir string, review bool, mirror io.Writer) (*Result, error) {
	return RunGrokModel(ctx, sess, prompt, effort, workdir, config.GrokModel, review, mirror)
}

// RunGrokModel executes a prompt with an explicit grok model, falling back to
// the default when model is empty — the contract the review pipeline relies on,
// where a session carries the concrete model to run or judge with. Unlike
// codex, the grok CLI does not read the prompt from stdin, so the composed
// prompt is handed over in a temp file, which also keeps it out of the process
// table. review selects the read-only sandbox used by review pipelines.
func RunGrokModel(ctx context.Context, sess *session.Session, prompt, effort, workdir, model string, review bool, mirror io.Writer) (*Result, error) {
	promptFile, err := os.CreateTemp("", "rival-grok-*.md")
	if err != nil {
		return nil, fmt.Errorf("%s runtime: create prompt file: %w", config.GrokLabel, err)
	}
	defer func() {
		if removeErr := os.Remove(promptFile.Name()); removeErr != nil && !os.IsNotExist(removeErr) {
			log.Warn().Err(removeErr).Str("file", promptFile.Name()).Msg("failed to remove grok prompt file")
		}
	}()

	if _, err := io.WriteString(promptFile, grokFullPrompt(prompt, workdir)); err != nil {
		_ = promptFile.Close()
		return nil, fmt.Errorf("%s runtime: write prompt file: %w", config.GrokLabel, err)
	}
	if err := promptFile.Close(); err != nil {
		return nil, fmt.Errorf("%s runtime: close prompt file: %w", config.GrokLabel, err)
	}

	args, err := grokRunArgs(model, promptFile.Name(), effort, workdir, review)
	if err != nil {
		return nil, fmt.Errorf("%s runtime: %w", config.GrokLabel, err)
	}

	// The prompt is already in the file; stdin carries nothing.
	result, err := grokSubprocess(ctx, sess, "grok", args, nil, "", mirror)
	if err != nil {
		return nil, fmt.Errorf("%s runtime: %s", config.GrokLabel, err.Error())
	}
	return result, nil
}

// grokFullPrompt composes the prompt exactly as the other executors do, so a
// grok run sees the same system prompt and workdir preamble as Sol or Claude.
func grokFullPrompt(prompt, workdir string) string {
	return config.SystemPrompt + "\n\n" + config.BuildWorkdirPreamble(workdir) + "\n" + prompt
}

// grokModelOrDefault resolves an optional model to grok's default, so a session
// that never recorded one still produces a valid `-m` instead of a bare flag.
func grokModelOrDefault(model string) string {
	if strings.TrimSpace(model) == "" {
		return config.GrokModel
	}
	return model
}

// grokRunArgs builds grok's argv. An empty model falls back to the default here
// rather than at the call site, so every entry point inherits the fallback.
func grokRunArgs(model, promptFile, effort, workdir string, review bool) ([]string, error) {
	mapped, err := GrokEffort(effort)
	if err != nil {
		return nil, err
	}

	args := []string{
		"--prompt-file", promptFile,
		"-m", grokModelOrDefault(model),
		"--effort", mapped,
		"--output-format", "plain",
		"--no-auto-update",
		"--yolo",
	}
	if workdir != "" {
		args = append(args, "--cwd", workdir)
	}
	if review {
		args = append(args, "--sandbox", "read-only")
	}
	return args, nil
}

// GrokEffort maps rival's effort menu onto grok-4.6's own low/medium/high.
// Levels above high clamp to high and levels below low clamp to low rather
// than failing a run over a level the model simply does not expose; an
// unrecognized value is still an error so typos do not silently downgrade.
// Exported so callers can record the clamped value on the session instead of
// the level the user asked for.
func GrokEffort(effort string) (string, error) {
	switch effort {
	case "low", "medium", "high":
		return effort, nil
	case "xhigh", "ultra", "max":
		return "high", nil
	case "minimal", "none":
		return "low", nil
	case "":
		return "", fmt.Errorf("effort is required for %s", config.GrokLabel)
	default:
		return "", fmt.Errorf("unsupported %s effort %q; use one of: low, medium, high", config.GrokLabel, effort)
	}
}
