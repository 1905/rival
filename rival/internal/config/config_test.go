package config

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestMaxConcurrent(t *testing.T) {
	tests := []struct {
		name string
		env  string
		want int
	}{
		{name: "unset uses two", env: "", want: 2},
		{name: "explicit override", env: "3", want: 3},
		{name: "zero falls back", env: "0", want: 2},
		{name: "negative falls back", env: "-1", want: 2},
		{name: "invalid falls back", env: "many", want: 2},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Setenv("RIVAL_MAX_CONCURRENT", tt.env)
			if got := MaxConcurrent(); got != tt.want {
				t.Errorf("MaxConcurrent()=%d, want %d", got, tt.want)
			}
		})
	}
}

func TestRunTimeout(t *testing.T) {
	tests := []struct {
		name string
		env  string
		want time.Duration
	}{
		{name: "unset → default", env: "", want: DefaultRunTimeout},
		{name: "explicit duration", env: "10m", want: 10 * time.Minute},
		{name: "zero disables", env: "0", want: 0},
		{name: "0s disables", env: "0s", want: 0},
		{name: "garbage → default", env: "banana", want: DefaultRunTimeout},
		{name: "negative → default", env: "-5m", want: DefaultRunTimeout},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Setenv("RIVAL_RUN_TIMEOUT", tt.env)
			if got := RunTimeout(); got != tt.want {
				t.Errorf("RunTimeout()=%v, want %v", got, tt.want)
			}
		})
	}
}

func TestMaxRunWait(t *testing.T) {
	// queue 30m + 2*run 30m + 5m margin = 95m by default.
	t.Run("default", func(t *testing.T) {
		t.Setenv("RIVAL_QUEUE_TIMEOUT", "")
		t.Setenv("RIVAL_RUN_TIMEOUT", "")
		if got, want := MaxRunWait(), 95*time.Minute; got != want {
			t.Errorf("MaxRunWait()=%v, want %v", got, want)
		}
	})
	t.Run("scales with configured timeouts", func(t *testing.T) {
		t.Setenv("RIVAL_QUEUE_TIMEOUT", "10m")
		t.Setenv("RIVAL_RUN_TIMEOUT", "20m")
		// 10 + 2*20 + 5 = 55m
		if got, want := MaxRunWait(), 55*time.Minute; got != want {
			t.Errorf("MaxRunWait()=%v, want %v", got, want)
		}
	})
	t.Run("run timeout disabled → queue + margin only", func(t *testing.T) {
		t.Setenv("RIVAL_QUEUE_TIMEOUT", "30m")
		t.Setenv("RIVAL_RUN_TIMEOUT", "0")
		if got, want := MaxRunWait(), 35*time.Minute; got != want {
			t.Errorf("MaxRunWait()=%v, want %v", got, want)
		}
	})
}

func TestWithRunTimeout(t *testing.T) {
	t.Run("disabled returns no deadline", func(t *testing.T) {
		t.Setenv("RIVAL_RUN_TIMEOUT", "0")
		ctx, cancel := WithRunTimeout(context.Background(), 1)
		defer cancel()
		if _, ok := ctx.Deadline(); ok {
			t.Error("expected no deadline when timeout disabled")
		}
	})
	t.Run("mult scales the budget", func(t *testing.T) {
		t.Setenv("RIVAL_RUN_TIMEOUT", "10m")
		ctx, cancel := WithRunTimeout(context.Background(), 2)
		defer cancel()
		dl, ok := ctx.Deadline()
		if !ok {
			t.Fatal("expected a deadline")
		}
		// ~20m out; allow slack for test execution time.
		if remaining := time.Until(dl); remaining < 19*time.Minute || remaining > 20*time.Minute {
			t.Errorf("deadline ~20m expected, got %v remaining", remaining)
		}
	})
	t.Run("mult<=0 returns no deadline", func(t *testing.T) {
		t.Setenv("RIVAL_RUN_TIMEOUT", "10m")
		ctx, cancel := WithRunTimeout(context.Background(), 0)
		defer cancel()
		if _, ok := ctx.Deadline(); ok {
			t.Error("expected no deadline when mult<=0")
		}
	})
}

func TestClaudeAuth(t *testing.T) {
	tests := []struct {
		name    string
		envAuth string
		envKey  string
		want    string
		wantErr string
	}{
		{name: "default is subscription", envAuth: "", envKey: "sk-ant-xxx", want: ClaudeAuthSubscription},
		{name: "explicit subscription", envAuth: "subscription", want: ClaudeAuthSubscription},
		{name: "sub shorthand", envAuth: "sub", want: ClaudeAuthSubscription},
		{name: "api with key", envAuth: "api", envKey: "sk-ant-xxx", want: ClaudeAuthAPI},
		{name: "api without key fails", envAuth: "api", envKey: "", wantErr: "ANTHROPIC_API_KEY is empty"},
		{name: "garbage fails", envAuth: "oauth2", wantErr: "invalid RIVAL_CLAUDE_AUTH"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Setenv("RIVAL_CLAUDE_AUTH", tt.envAuth)
			t.Setenv("ANTHROPIC_API_KEY", tt.envKey)
			got, err := ClaudeAuth()
			if tt.wantErr != "" {
				if err == nil || !strings.Contains(err.Error(), tt.wantErr) {
					t.Fatalf("want error containing %q, got %v", tt.wantErr, err)
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if got != tt.want {
				t.Errorf("got %q, want %q", got, tt.want)
			}
		})
	}
}

func TestEngineLabel(t *testing.T) {
	cases := []struct{ cli, model, want string }{
		{"codex", GPT56SolModel, SolLabel},
		{"codex", "retired-sol-id", SolLabel},
		{"codex", "", SolLabel},
		{"claude", ClaudeModel, ClaudeLabel},
		{"claude", "retired-claude-id", "retired-model"},
		{"claude", "", "retired-model"},
		{"claude", "retired-model-id", "retired-model"},
		{"claude", ClaudeModel, ClaudeLabel},
		{"claude", "", "retired-model"},
		{"opencode", KimiModel, K3Label},
		{"opencode", "provider/retired-model-id", "retired-model"},
		{"opencode", "", "retired-model"},
		{"grok", GrokModel, GrokLabel},
		{"grok", "", GrokLabel},
	}
	for _, c := range cases {
		if got := EngineLabel(c.cli, c.model); got != c.want {
			t.Errorf("EngineLabel(%q,%q) = %q, want %q", c.cli, c.model, got, c.want)
		}
	}
}

func TestModelLabelOnlyExposesSupportedModels(t *testing.T) {
	cases := []struct{ model, want string }{
		{GPT56SolModel, SolLabel},
		{SolLabel, SolLabel},
		{ClaudeModel, ClaudeLabel},
		{ClaudeLabel, ClaudeLabel},
		{KimiModel, K3Label},
		{K3Label, K3Label},
		{GrokModel, GrokLabel},
		{GrokLabel, GrokLabel},
		{"custom/model", "retired-model"},
		{"", "retired-model"},
	}
	for _, c := range cases {
		if got := ModelLabel(c.model); got != c.want {
			t.Errorf("ModelLabel(%q) = %q, want %q", c.model, got, c.want)
		}
	}
}

func TestPublicRuntimeLogNormalizesOnlyRuntimeMetadata(t *testing.T) {
	raw := "OpenAI Codex v0.130.0\n--------\nmodel: retired-sol-id\nprovider: openai\n--------\nuser\n" +
		"inspect rival/cmd/command_codex.go\n" +
		"=== REVIEW FROM codex (retired-sol-id) [role: bug_hunter] ===\n"
	got := PublicRuntimeLog("codex", "retired-sol-id", raw)
	for _, want := range []string{
		"Sol runtime v0.130.0",
		"model: sol",
		"=== REVIEW FROM sol [role: bug_hunter] ===",
		"rival/cmd/command_codex.go",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("public log missing %q:\n%s", want, got)
		}
	}
	for _, forbidden := range []string{"OpenAI Codex", "model: retired-sol-id", "REVIEW FROM codex"} {
		if strings.Contains(got, forbidden) {
			t.Errorf("public log exposes %q:\n%s", forbidden, got)
		}
	}
}

func TestPublicRuntimeLogHidesRetiredRuntimeIdentities(t *testing.T) {
	tests := []struct {
		name  string
		cli   string
		model string
		raw   string
	}{
		{
			name:  "claude",
			cli:   "claude",
			model: "retired-claude-id",
			raw:   "Claude Code v1\n--------\nmodel: retired-claude-id\n--------\n=== REVIEW FROM claude (retired-claude-id) [role: bug_hunter] ===\n",
		},
		{
			name:  "opencode",
			cli:   "opencode",
			model: "custom/model",
			raw:   "=== REVIEW FROM opencode (custom/model) [role: bug_hunter] ===\n",
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := PublicRuntimeLog(tc.cli, tc.model, tc.raw)
			if !strings.Contains(got, "retired-model") {
				t.Fatalf("public log missing retired-model label:\n%s", got)
			}
			if strings.Contains(got, tc.model) || strings.Contains(got, "REVIEW FROM "+tc.cli) {
				t.Fatalf("public log exposes retired runtime identity:\n%s", got)
			}
		})
	}
}

func TestPublicRuntimeLogLabelsK3ReviewHeader(t *testing.T) {
	raw := "=== REVIEW FROM opencode (" + KimiModel + ") [role: bug_hunter] ===\n"
	got := PublicRuntimeLog("opencode", KimiModel, raw)
	want := "=== REVIEW FROM " + K3Label + " [role: bug_hunter] ==="
	if !strings.Contains(got, want) || strings.Contains(got, "opencode") || strings.Contains(got, KimiModel) {
		t.Fatalf("public K3 header = %q, want %q", got, want)
	}
}

func TestPublicRuntimeErrorUsesPublicModelName(t *testing.T) {
	got := PublicRuntimeError("codex", GPT56SolModel, "Codex CLI failed for gpt-5.6-sol; run codex login")
	if strings.Contains(strings.ToLower(got), "codex") || strings.Contains(got, GPT56SolModel) || !strings.Contains(strings.ToLower(got), SolLabel) {
		t.Fatalf("public error was not normalized: %q", got)
	}
}

func TestResolveEffortPrecedenceAndModelDefaults(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	userConfig = nil
	userConfigErr = nil
	defaults := []struct {
		model string
		want  string
	}{
		{CodexModel, "xhigh"},
		{KimiModel, "max"},
		{ClaudeModel, "medium"},
	}
	for _, tc := range defaults {
		got, err := ResolveEffort(tc.model, "", "")
		if err != nil {
			t.Fatal(err)
		}
		if got != tc.want {
			t.Errorf("ResolveEffort(%s) = %q, want %q", ModelLabel(tc.model), got, tc.want)
		}
	}

	userConfig = &UserConfig{Efforts: map[string]string{
		GrokLabel:   "low",
		"kimi-k3":   "max",
		ClaudeLabel: "high",
	}}
	if got, _ := ResolveEffort(GrokModel, "", "high"); got != "low" {
		t.Errorf("configured grok effort = %q, want low", got)
	}
	if got, _ := ResolveEffort(GrokModel, "medium", "low"); got != "medium" {
		t.Errorf("explicit effort = %q, want medium", got)
	}
	if got, _ := ResolveEffort(KimiModel, "low", "low"); got != "max" {
		t.Errorf("fixed K3 effort = %q, want max", got)
	}

	userConfig = &UserConfig{}
	if got, _ := ResolveEffort(GrokModel, "", "low"); got != "low" {
		t.Errorf("surface fallback = %q, want low", got)
	}
	// Claude's medium pin outranks a surface fallback; -re still wins.
	if got, _ := ResolveEffort(ClaudeModel, "", "high"); got != "medium" {
		t.Errorf("claude pinned effort = %q, want medium", got)
	}
	if got, _ := ResolveEffort(ClaudeModel, "xhigh", "high"); got != "xhigh" {
		t.Errorf("claude explicit effort = %q, want xhigh", got)
	}
}

func TestGrokEffortDefaultsAndConfiguredOverride(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	userConfig = nil
	userConfigErr = nil
	if got := DefaultEffortForModel(GrokModel); got != "high" {
		t.Errorf("DefaultEffortForModel(grok) = %q, want high", got)
	}
	if got, err := ResolveEffort(GrokModel, "", DefaultReviewEffort); err != nil || got != "high" {
		t.Errorf("ResolveEffort(grok) = %q, %v; want high", got, err)
	}
	if got, err := ResolveEffort(GrokModel, "medium", ""); err != nil || got != "medium" {
		t.Errorf("explicit grok effort = %q, %v; want medium", got, err)
	}
	if _, err := ResolveEffort(GrokModel, "max", ""); err == nil {
		t.Error("grok accepted max, want invalid effort error")
	}

	userConfig = &UserConfig{Efforts: map[string]string{GrokLabel: "low"}}
	if got, err := ResolveEffort(GrokModel, "", DefaultReviewEffort); err != nil || got != "low" {
		t.Errorf("configured grok effort = %q, %v; want low", got, err)
	}
	if got := DefaultEffortForModel(GrokModel); got != "low" {
		t.Errorf("DefaultEffortForModel(grok) with config = %q, want low", got)
	}
}

func TestGrokIsAValidConfiguredEffortModel(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	home := t.TempDir()
	t.Setenv("HOME", home)
	dir := filepath.Join(home, ".rival")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte("efforts:\n  grok: low\n"), 0600); err != nil {
		t.Fatal(err)
	}
	LoadUserConfig()
	if err := UserConfigError(); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got := DefaultEffortForModel(GrokModel); got != "low" {
		t.Errorf("loaded grok effort = %q, want low", got)
	}
}

func TestGrokIsEnumeratedInEffortModelErrors(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	home := t.TempDir()
	t.Setenv("HOME", home)
	dir := filepath.Join(home, ".rival")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte("efforts:\n  mystery: high\n"), 0600); err != nil {
		t.Fatal(err)
	}
	LoadUserConfig()
	err := UserConfigError()
	if err == nil {
		t.Fatal("unknown effort model was accepted")
	}
	if !strings.Contains(err.Error(), GrokLabel) {
		t.Errorf("error %q does not enumerate %q", err, GrokLabel)
	}
}

func TestGrokConcreteModelIDIsNeverExposed(t *testing.T) {
	raw := "=== REVIEW FROM grok (" + GrokModel + ") [role: bug_hunter] ===\nchecked " + GrokModel + "\n"
	got := PublicRuntimeLog("grok", GrokModel, raw)
	want := "=== REVIEW FROM " + GrokLabel + " [role: bug_hunter] ==="
	if !strings.Contains(got, want) {
		t.Fatalf("public grok header = %q, want %q", got, want)
	}
	if strings.Contains(got, GrokModel) {
		t.Fatalf("public log exposes the concrete grok model id:\n%s", got)
	}
}

func TestLoadUserConfigValidatesEffortMap(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	tests := []struct {
		name    string
		body    string
		wantErr string
	}{
		{
			name: "valid",
			// sol is a removed model: its old entry is dropped, not an error.
			body: "efforts:\n  sol: low\n  codex: ultra\n  kimi-k3: max\n  claude: medium\n",
		},
		{
			name:    "unknown model",
			body:    "efforts:\n  mystery: high\n",
			wantErr: "invalid effort model",
		},
		{
			name:    "invalid k3 level",
			body:    "efforts:\n  kimi-k3: high\n",
			wantErr: "use one of: max",
		},
		{
			name:    "invalid yaml",
			body:    "efforts: [",
			wantErr: "parse ",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			dir := filepath.Join(home, ".rival")
			if err := os.MkdirAll(dir, 0700); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(tc.body), 0600); err != nil {
				t.Fatal(err)
			}
			LoadUserConfig()
			err := UserConfigError()
			if tc.wantErr == "" {
				if err != nil {
					t.Fatalf("unexpected error: %v", err)
				}
				if got := DefaultEffortForModel(CodexModel); got != "ultra" {
					t.Errorf("loaded codex effort = %q, want ultra", got)
				}
				if _, ok := userConfig.Efforts[SolLabel]; ok {
					t.Error("removed sol effort entry was kept")
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
				t.Fatalf("error = %v, want substring %q", err, tc.wantErr)
			}
		})
	}
}

func TestLoadUserConfigReportsUnreadableConfigPath(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	home := t.TempDir()
	t.Setenv("HOME", home)
	path := filepath.Join(home, ".rival", "config.yaml")
	if err := os.MkdirAll(path, 0o700); err != nil {
		t.Fatal(err)
	}

	LoadUserConfig()
	err := UserConfigError()
	if err == nil {
		t.Fatal("directory at config path was silently ignored")
	}
	if message := err.Error(); !strings.Contains(message, "read "+path) {
		t.Fatalf("error = %q, want config path", message)
	}
}

// A config.yaml written for the removed megareview (a roster key, a judge role
// override) must keep loading: obsolete keys are ignored, never a hard failure.
func TestLoadUserConfigIgnoresObsoleteMegareviewKeys(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	home := t.TempDir()
	t.Setenv("HOME", home)
	dir := filepath.Join(home, ".rival")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	body := "review:\n  models: [codex, k3]\nroles:\n  consilium: judge prompt\n  bug_hunter: custom hunter\n"
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	LoadUserConfig()
	if err := UserConfigError(); err != nil {
		t.Fatalf("obsolete megareview keys broke config loading: %v", err)
	}
	if got, ok := RolePromptOverride("bug_hunter"); !ok || got != "custom hunter" {
		t.Fatalf("bug_hunter override = %q, %v; want the configured prompt", got, ok)
	}
}

func TestAutoFixCriticalHighDefaultsOff(t *testing.T) {
	oldConfig, oldErr := userConfig, userConfigErr
	t.Cleanup(func() {
		userConfig, userConfigErr = oldConfig, oldErr
	})

	home := t.TempDir()
	t.Setenv("HOME", home)
	dir := filepath.Join(home, ".rival")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ body, want string }{
		{"", "off"},
		{"efforts:\n  codex: high\n", "off"},
		{"auto_fix_critical_high: false\n", "off"},
		{"auto_fix_critical_high:\n", "off"},
		{"auto_fix_critical_high: true\n", "critical+high"},
	} {
		if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(tc.body), 0o600); err != nil {
			t.Fatal(err)
		}
		LoadUserConfig()
		if err := UserConfigError(); err != nil {
			t.Fatalf("%q: %v", tc.body, err)
		}
		if got := AutoFixPolicy(); got != tc.want {
			t.Fatalf("%q: AutoFixPolicy() = %q, want %q", tc.body, got, tc.want)
		}
	}
}
