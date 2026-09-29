package executor

import (
	"context"
	"io"
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
)

func TestGrokEffort(t *testing.T) {
	cases := []struct {
		effort  string
		want    string
		wantErr bool
	}{
		{effort: "low", want: "low"},
		{effort: "medium", want: "medium"},
		{effort: "high", want: "high"},
		// grok-4.6 has no reasoning level above high; rival's richer menu clamps down.
		{effort: "xhigh", want: "high"},
		{effort: "ultra", want: "high"},
		{effort: "max", want: "high"},
		{effort: "minimal", want: "low"},
		{effort: "none", want: "low"},
		{effort: "", wantErr: true},
		{effort: "turbo", wantErr: true},
	}

	for _, tc := range cases {
		t.Run(tc.effort, func(t *testing.T) {
			got, err := GrokEffort(tc.effort)
			if tc.wantErr {
				if err == nil {
					t.Fatalf("effort %q was accepted, got %q", tc.effort, got)
				}
				return
			}
			if err != nil {
				t.Fatalf("effort %q rejected: %v", tc.effort, err)
			}
			if got != tc.want {
				t.Fatalf("effort %q mapped to %q, want %q", tc.effort, got, tc.want)
			}
		})
	}
}

func TestGrokRunArgs_AlwaysPassesPromptFileAndRuntimeFlags(t *testing.T) {
	args, err := grokRunArgs(config.GrokModel, "/tmp/rival-grok-1.md", "high", "", false)
	if err != nil {
		t.Fatalf("grokRunArgs: %v", err)
	}
	joined := strings.Join(args, " ")

	for _, want := range []string{
		"--prompt-file /tmp/rival-grok-1.md",
		"-m " + config.GrokModel,
		"--effort high",
		"--output-format plain",
		"--no-auto-update",
		"--yolo",
	} {
		if !strings.Contains(joined, want) {
			t.Errorf("args missing %q: %s", want, joined)
		}
	}
	// The prompt travels by file, never inline: a bare -p would both duplicate it
	// and expose it in the process table.
	for _, arg := range args {
		if arg == "-p" {
			t.Errorf("args used a bare -p instead of --prompt-file: %s", joined)
		}
	}
	if strings.Contains(joined, "--cwd") {
		t.Errorf("empty workdir must not emit --cwd: %s", joined)
	}
	if strings.Contains(joined, "--sandbox") {
		t.Errorf("raw run must not emit a sandbox flag: %s", joined)
	}
}

func TestGrokRunArgs_WorkdirAndReviewSandbox(t *testing.T) {
	cases := []struct {
		name        string
		workdir     string
		review      bool
		wantCwd     bool
		wantSandbox bool
	}{
		{name: "raw_no_workdir"},
		{name: "raw_with_workdir", workdir: "/repo", wantCwd: true},
		{name: "review_no_workdir", review: true, wantSandbox: true},
		{name: "review_with_workdir", workdir: "/repo", review: true, wantCwd: true, wantSandbox: true},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			args, err := grokRunArgs(config.GrokModel, "/tmp/p.md", "medium", tc.workdir, tc.review)
			if err != nil {
				t.Fatalf("grokRunArgs: %v", err)
			}
			joined := strings.Join(args, " ")

			gotCwd := strings.Contains(joined, "--cwd "+tc.workdir)
			if gotCwd != tc.wantCwd {
				t.Errorf("--cwd %q present=%v, want %v: %s", tc.workdir, gotCwd, tc.wantCwd, joined)
			}
			gotSandbox := strings.Contains(joined, "--sandbox read-only")
			if gotSandbox != tc.wantSandbox {
				t.Errorf("--sandbox read-only present=%v, want %v: %s", gotSandbox, tc.wantSandbox, joined)
			}
		})
	}
}

// The consilium judge hands the session's recorded model to the adapter, so an
// explicit model must reach argv and an empty one must fall back to grok's
// default rather than emitting a bare "-m".
func TestGrokModelOrDefault(t *testing.T) {
	if got := grokModelOrDefault(""); got != config.GrokModel {
		t.Errorf("grokModelOrDefault(\"\") = %q, want %q", got, config.GrokModel)
	}
	if got := grokModelOrDefault("   "); got != config.GrokModel {
		t.Errorf("grokModelOrDefault(blank) = %q, want %q", got, config.GrokModel)
	}
	if got := grokModelOrDefault("grok-4.5-fast"); got != "grok-4.5-fast" {
		t.Errorf("grokModelOrDefault(explicit) = %q, want the explicit model", got)
	}
}

func TestGrokRunArgs_ThreadsExplicitModel(t *testing.T) {
	args, err := grokRunArgs("grok-4.5-fast", "/tmp/p.md", "high", "/repo", true)
	if err != nil {
		t.Fatalf("grokRunArgs: %v", err)
	}
	// Compare the -m value exactly: config.GrokModel is a prefix of the test
	// model, so a substring check would pass even if the default won.
	if got := argValue(args, "-m"); got != "grok-4.5-fast" {
		t.Errorf("-m = %q, want the explicit model: %v", got, args)
	}

	fallback, err := grokRunArgs("", "/tmp/p.md", "high", "/repo", true)
	if err != nil {
		t.Fatalf("grokRunArgs: %v", err)
	}
	if got := argValue(fallback, "-m"); got != config.GrokModel {
		t.Errorf("empty model -m = %q, want the default %q", got, config.GrokModel)
	}
}

// RunGrokModel is the entry point the review pipeline calls, so the model must
// survive all the way to argv there — not merely in grokRunArgs, which an
// intermediate hardcode would bypass.
func TestRunGrokModel_ThreadsModelToArgv(t *testing.T) {
	for _, tc := range []struct{ name, model, want string }{
		{"explicit model", "grok-4.5-fast", "grok-4.5-fast"},
		{"empty model falls back", "", config.GrokModel},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var gotArgs []string
			restore := grokSubprocess
			grokSubprocess = func(_ context.Context, _ *session.Session, binary string, args []string, _ []string, _ string, _ io.Writer, _ ...string) (*Result, error) {
				if binary != "grok" {
					t.Errorf("spawned %q, want grok", binary)
				}
				gotArgs = args
				return &Result{}, nil
			}
			t.Cleanup(func() { grokSubprocess = restore })

			if _, err := RunGrokModel(context.Background(), nil, "prompt", "high", t.TempDir(), tc.model, true, nil); err != nil {
				t.Fatalf("RunGrokModel: %v", err)
			}
			if got := argValue(gotArgs, "-m"); got != tc.want {
				t.Errorf("-m = %q, want %q: %v", got, tc.want, gotArgs)
			}
		})
	}
}

// The default-model wrapper must keep sending grok's default.
func TestRunGrok_SendsDefaultModel(t *testing.T) {
	var gotArgs []string
	restore := grokSubprocess
	grokSubprocess = func(_ context.Context, _ *session.Session, _ string, args []string, _ []string, _ string, _ io.Writer, _ ...string) (*Result, error) {
		gotArgs = args
		return &Result{}, nil
	}
	t.Cleanup(func() { grokSubprocess = restore })

	if _, err := RunGrok(context.Background(), nil, "prompt", "high", t.TempDir(), false, nil); err != nil {
		t.Fatalf("RunGrok: %v", err)
	}
	if got := argValue(gotArgs, "-m"); got != config.GrokModel {
		t.Errorf("-m = %q, want %q", got, config.GrokModel)
	}
}

// argValue returns the value following flag in an argv slice.
func argValue(args []string, flag string) string {
	for i, arg := range args {
		if arg == flag && i+1 < len(args) {
			return args[i+1]
		}
	}
	return ""
}

func TestGrokRunArgs_PropagatesEffortError(t *testing.T) {
	args, err := grokRunArgs(config.GrokModel, "/tmp/p.md", "turbo", "/repo", true)
	if err == nil {
		t.Fatalf("invalid effort was accepted: %v", args)
	}
	if args != nil {
		t.Fatalf("invalid effort returned args: %v", args)
	}
}

func TestGrokFullPrompt_MatchesSharedComposition(t *testing.T) {
	const prompt = "review this diff"
	want := config.SystemPrompt + "\n\n" + config.BuildWorkdirPreamble("/repo") + "\n" + prompt
	if got := grokFullPrompt(prompt, "/repo"); got != want {
		t.Fatalf("grokFullPrompt mismatch:\ngot:  %q\nwant: %q", got, want)
	}
}
