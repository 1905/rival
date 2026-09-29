package executor

import (
	"context"
	"io"
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
)

func TestCodexRunArgs_UsesExplicitModelAndEffort(t *testing.T) {
	for _, effort := range []string{"high", "ultra"} {
		t.Run(effort, func(t *testing.T) {
			joined := strings.Join(codexRunArgs(config.CodexModel, effort, "/repo"), " ")
			if !strings.Contains(joined, "-m "+config.CodexModel) {
				t.Fatalf("args do not select %s: %s", config.CodexModel, joined)
			}
			if !strings.Contains(joined, "model_reasoning_effort="+effort) {
				t.Fatalf("args do not preserve effort %s: %s", effort, joined)
			}
			if !strings.Contains(joined, "--sandbox read-only") {
				t.Fatalf("args lost read-only sandbox: %s", joined)
			}
		})
	}
}

func TestRunCodexModelRejectsUnsupportedModel(t *testing.T) {
	result, err := RunCodexModel(
		context.Background(),
		nil,
		"review",
		"high",
		"/repo",
		"retired-model",
		io.Discard,
	)
	if err == nil {
		t.Fatal("unsupported model was accepted")
	}
	if result != nil {
		t.Fatalf("unsupported model returned a result: %#v", result)
	}
}

// Sol was removed and an empty model no longer means Codex: both are
// rejected before any process starts.
func TestRunCodexModelRejectsSolAndEmpty(t *testing.T) {
	for _, model := range []string{config.GPT56SolModel, ""} {
		result, err := RunCodexModel(context.Background(), nil, "review", "high", "/repo", model, io.Discard)
		if err == nil || !strings.Contains(err.Error(), "unsupported codex model") {
			t.Fatalf("model %q: err = %v, want unsupported codex model", model, err)
		}
		if result != nil {
			t.Fatalf("model %q returned a result: %#v", model, result)
		}
	}
}

// The codex runtime takes ultra as its own reasoning level, distinct from
// xhigh. Unifying the ladder must not alias the two: the value reaches the
// runtime verbatim.
func TestCodexPassesUltraAndXhighThroughUnaliased(t *testing.T) {
	for _, effort := range []string{"xhigh", "ultra"} {
		args := codexRunArgs(config.CodexModel, effort, "/tmp")
		want := "model_reasoning_effort=" + effort
		if !strings.Contains(strings.Join(args, " "), want) {
			t.Errorf("codex args for %q missing %q: %v", effort, want, args)
		}
	}
}
