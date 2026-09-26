package executor

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
)

func TestClaudeReviewTransportRestrictions(t *testing.T) {
	for _, mode := range []string{"review", "plan", "antislop", "raw"} {
		t.Run(mode, func(t *testing.T) {
			home, bin, repo := t.TempDir(), t.TempDir(), t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("PATH", bin)
			t.Setenv("CLAUDECODE", "1")
			t.Setenv("RIVAL_CLAUDE_AUTH", "subscription")
			t.Setenv("ANTHROPIC_API_KEY", "should-not-reach-child")
			fake := "#!/bin/sh\n[ -z \"$CLAUDECODE\" ] || exit 10\n[ -z \"$ANTHROPIC_API_KEY\" ] || exit 11\nprintf '%s\\n' \"$@\"\n"
			if err := os.WriteFile(filepath.Join(bin, "claude"), []byte(fake), 0700); err != nil {
				t.Fatal(err)
			}
			sess, err := session.NewQueued("claude", mode, config.ClaudeModel, "medium", repo, "review", "", "")
			if err != nil {
				t.Fatal(err)
			}
			var out bytes.Buffer
			result, err := RunClaude(context.Background(), sess, "review", "medium", repo, &out)
			if err != nil {
				t.Fatal(err)
			}
			if result.ExitCode != 0 {
				t.Fatalf("child exit %d", result.ExitCode)
			}
			args := strings.Split(strings.TrimSuffix(out.String(), "\n"), "\n")
			if !slices.Contains(args, config.ClaudeModel) {
				t.Fatal("Claude model not selected")
			}
			if mode == "raw" {
				if !slices.Contains(args, "--dangerously-skip-permissions") || slices.Contains(args, "--tools") {
					t.Fatal("raw prompt behavior changed")
				}
				return
			}
			if slices.Contains(args, "--dangerously-skip-permissions") {
				t.Fatal("review bypasses permissions")
			}
			for flag, want := range map[string]string{"--tools": "Read,Glob,Grep", "--permission-mode": "dontAsk", "--disallowedTools": "mcp__*", "--setting-sources": "", "--settings": `{"disableAllHooks":true}`, "--mcp-config": `{"mcpServers":{}}`} {
				i := slices.Index(args, flag)
				if i < 0 || i+1 >= len(args) || args[i+1] != want {
					t.Fatalf("missing restriction %s %s in %v", flag, want, args)
				}
			}
			if !slices.Contains(args, "--safe-mode") || !slices.Contains(args, "--strict-mcp-config") {
				t.Fatal("review loads customizations")
			}
		})
	}
}

func TestClaudeDockerReviewMountIsReadOnly(t *testing.T) {
	bin, home, repo := t.TempDir(), t.TempDir(), t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("PATH", bin)
	t.Setenv(config.ClaudeDockerTokenEnv, "fixture-token")
	if err := os.WriteFile(filepath.Join(bin, "docker"), []byte("#!/bin/sh\nprintf '%s\\n' \"$@\"\n"), 0700); err != nil {
		t.Fatal(err)
	}
	sess, err := session.NewQueued("claude", "review", config.ClaudeModel, "medium", repo, "review", "", "")
	if err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	result, err := RunClaude(context.Background(), sess, "review", "medium", repo, &out)
	if err != nil {
		t.Fatal(err)
	}
	if result.ExitCode != 0 || !strings.Contains(out.String(), repo+":/workspace:ro\n") || !strings.Contains(out.String(), "Read,Glob,Grep") {
		t.Fatalf("unsafe Docker review: %s", out.String())
	}
}

func TestClaudeAuthHint(t *testing.T) {
	writeLog := func(t *testing.T, content string) string {
		t.Helper()
		p := filepath.Join(t.TempDir(), "run.log")
		if err := os.WriteFile(p, []byte(content), 0600); err != nil {
			t.Fatal(err)
		}
		return p
	}

	tests := []struct {
		name    string
		log     string
		envAuth string
		envKey  string
		want    string // substring; "" = no hint
	}{
		{name: "credit balance, sub mode", log: "Credit balance is too low", envAuth: "", want: "subscription auth failed"},
		{name: "credit balance, api mode", log: "Credit balance is too low", envAuth: "api", envKey: "sk-x", want: "API auth failed"},
		{name: "login prompt", log: "Please run /login to continue", envAuth: "", want: "subscription auth failed"},
		{name: "invalid key", log: "Invalid API key provided", envAuth: "api", envKey: "sk-x", want: "API auth failed"},
		{name: "model failure is not auth", log: "model overloaded, retry later", envAuth: "", want: ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Setenv("RIVAL_CLAUDE_AUTH", tt.envAuth)
			t.Setenv("ANTHROPIC_API_KEY", tt.envKey)
			got := ClaudeAuthHint(writeLog(t, tt.log))
			if tt.want == "" {
				if got != "" {
					t.Fatalf("want no hint, got %q", got)
				}
				return
			}
			if !strings.Contains(got, tt.want) {
				t.Errorf("hint %q does not contain %q", got, tt.want)
			}
		})
	}

	t.Run("missing log file", func(t *testing.T) {
		if got := ClaudeAuthHint(filepath.Join(t.TempDir(), "nope.log")); got != "" {
			t.Errorf("want empty, got %q", got)
		}
	})
}

func TestSetClaudeTransportModePreservesPlanTask(t *testing.T) {
	plan := &session.Session{Mode: "plan"}
	setClaudeTransportMode(plan, "native")
	if plan.Mode != "plan" {
		t.Fatalf("plan mode = %q, want plan", plan.Mode)
	}

	standalone := &session.Session{Mode: "raw"}
	setClaudeTransportMode(standalone, "docker")
	if standalone.Mode != "docker" {
		t.Fatalf("standalone mode = %q, want docker", standalone.Mode)
	}
}

// A task session is named by its mode in both dashboards. Writing the
// transport over that mode labels the run for its whole live execution, which
// is how antislop runs appeared as megareviews while running.
func TestSetClaudeTransportModePreservesTaskModes(t *testing.T) {
	tests := []struct {
		name string
		mode string
		want string
	}{
		{"plan survives", session.ModePlan, session.ModePlan},
		{"antislop survives", session.ModeAntislop, session.ModeAntislop},
		{"raw records transport", "raw", "native"},
		{"review records transport", "review", "native"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sess := &session.Session{Mode: tt.mode}
			setClaudeTransportMode(sess, "native")
			if sess.Mode != tt.want {
				t.Errorf("mode = %q, want %q", sess.Mode, tt.want)
			}
		})
	}
}
