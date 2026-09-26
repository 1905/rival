package review

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
)

// realPlanJSON is a minimal valid plan payload ParsePlanOutput accepts.
const realPlanJSON = `{"summary":"ok plan","rating":7,"findings":[]}`

func TestCodexPlanUsesCodexRuntimeAndStructuredOutput(t *testing.T) {
	loadPlanTestConfig(t, "")
	bin, repo := t.TempDir(), t.TempDir()
	t.Setenv("PATH", bin)
	argsFile := filepath.Join(repo, "args.txt")
	t.Setenv("RIVAL_TEST_ARGS", argsFile)
	script := "#!/bin/sh\nif [ \"$1\" = login ]; then exit 0; fi\nprintf '%s\\n' \"$@\" > \"$RIVAL_TEST_ARGS\"\nprintf '%s\\n' '" + realPlanJSON + "'\n"
	if err := os.WriteFile(filepath.Join(bin, "codex"), []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	result, err := RunPlanReview(context.Background(), filepath.Join(repo, "plan.md"), "", repo, "codex-proof", true, []string{"codex"})
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Results) != 1 || result.Results[0].Model != config.CodexModel || result.Results[0].Parsed == nil || result.Results[0].Parsed.Rating != 7 {
		t.Fatalf("wrong Codex result: %+v", result)
	}
	args, err := os.ReadFile(argsFile)
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"-m\n" + config.CodexModel, "model_reasoning_effort=xhigh", "--sandbox\nread-only"} {
		if !strings.Contains(string(args), want) {
			t.Fatalf("runtime arguments missing %q: %s", want, args)
		}
	}
	if strings.Contains(string(args), config.GPT56SolModel) {
		t.Fatal("Codex plan ran Sol")
	}
	if !strings.Contains(FormatPlanResult(result, "plan.md"), "7/10") {
		t.Fatal("plan result lost structured rating")
	}
}

func TestAssemblePlanResults_AllFailed(t *testing.T) {
	batch := []planCLIRun{
		{CLI: "codex", ExitCode: 1},
		{CLI: "claude", Err: errString("boom")},
	}
	if _, err := assemblePlanResults(batch, nil); err == nil {
		t.Fatal("expected error when every CLI fails")
	}
}

func TestAssemblePlanResults_OneSkippedOneOK(t *testing.T) {
	batch := []planCLIRun{
		{CLI: "codex", Model: config.GPT56SolModel, Raw: realPlanJSON, ExitCode: 0},
	}
	// claude was unavailable at preflight → pre-run skipped list.
	pre := []SkippedCLI{{CLI: "claude", Reason: "claude not found"}}

	res, err := assemblePlanResults(batch, pre)
	if err != nil {
		t.Fatalf("assemblePlanResults: %v", err)
	}
	if len(res.Results) != 1 || res.Results[0].CLI != "codex" {
		t.Fatalf("want 1 codex result, got %+v", res.Results)
	}
	if res.Results[0].Parsed == nil || res.Results[0].Parsed.Rating != 7 {
		t.Fatalf("codex result not parsed: %+v", res.Results[0])
	}
	if len(res.Skipped) != 1 || res.Skipped[0].CLI != "claude" {
		t.Fatalf("want claude skipped preserved, got %+v", res.Skipped)
	}
}

func TestAssemblePlanResults_NonzeroExitSkips(t *testing.T) {
	batch := []planCLIRun{
		{CLI: "codex", Model: config.GPT56SolModel, Raw: realPlanJSON, ExitCode: 0},
		{CLI: "claude", Model: config.ClaudeModel, Raw: "partial", ExitCode: 2},
	}
	res, err := assemblePlanResults(batch, nil)
	if err != nil {
		t.Fatalf("assemblePlanResults: %v", err)
	}
	if len(res.Results) != 1 || res.Results[0].CLI != "codex" {
		t.Fatalf("want only codex kept, got %+v", res.Results)
	}
	if len(res.Skipped) != 1 || !strings.Contains(res.Skipped[0].Reason, "exited with code 2") {
		t.Fatalf("want claude skipped with exit reason, got %+v", res.Skipped)
	}
}

func TestAssemblePlanResults_QuotaSkips(t *testing.T) {
	batch := []planCLIRun{
		{CLI: "codex", Model: config.GPT56SolModel, Raw: "error: insufficient_quota", ExitCode: 0},
		{CLI: "claude", Model: config.ClaudeModel, Raw: realPlanJSON, ExitCode: 0},
	}
	res, err := assemblePlanResults(batch, nil)
	if err != nil {
		t.Fatalf("assemblePlanResults: %v", err)
	}
	if len(res.Results) != 1 || res.Results[0].CLI != "claude" {
		t.Fatalf("want only claude kept (codex quota'd), got %+v", res.Results)
	}
	if len(res.Skipped) != 1 || !strings.Contains(res.Skipped[0].Reason, "429") {
		t.Fatalf("want codex quota-skipped, got %+v", res.Skipped)
	}
}

func TestAssemblePlanResults_ParseFailKeepsRaw(t *testing.T) {
	// Exit 0, no quota, but output has no parseable plan payload → keep Raw, nil Parsed.
	batch := []planCLIRun{
		{CLI: "codex", Model: config.GPT56SolModel, Raw: "just some prose, no json", ExitCode: 0},
	}
	res, err := assemblePlanResults(batch, nil)
	if err != nil {
		t.Fatalf("assemblePlanResults: %v", err)
	}
	if len(res.Results) != 1 {
		t.Fatalf("want 1 result kept on parse failure, got %+v", res.Results)
	}
	if res.Results[0].Parsed != nil {
		t.Fatalf("want nil Parsed on parse failure, got %+v", res.Results[0].Parsed)
	}
	if res.Results[0].Raw != "just some prose, no json" {
		t.Fatalf("raw not preserved: %q", res.Results[0].Raw)
	}
}

func TestAssemblePlanResults_EmptyOutputSkips(t *testing.T) {
	// An exit-0 run that wrote nothing must be skipped, not treated as a
	// successful (but empty) plan review.
	batch := []planCLIRun{
		{CLI: "claude", Model: config.ClaudeModel, Raw: "   \n  ", ExitCode: 0},
		{CLI: "codex", Model: config.GPT56SolModel, Raw: realPlanJSON, ExitCode: 0},
	}
	res, err := assemblePlanResults(batch, nil)
	if err != nil {
		t.Fatalf("assemblePlanResults: %v", err)
	}
	if len(res.Results) != 1 || res.Results[0].CLI != "codex" {
		t.Fatalf("want only codex kept (claude empty), got %+v", res.Results)
	}
	if len(res.Skipped) != 1 || res.Skipped[0].CLI != "claude" {
		t.Fatalf("want claude skipped for empty output, got %+v", res.Skipped)
	}
}

func TestPlanEngineLabel(t *testing.T) {
	if got := config.EngineLabel("codex", config.GPT56SolModel); got != config.SolLabel {
		t.Errorf("sol label = %q, want %q", got, config.SolLabel)
	}
	if got := config.EngineLabel("claude", config.ClaudeModel); got != config.ClaudeLabel {
		t.Errorf("claude label = %q, want %q", got, config.ClaudeLabel)
	}
}

func TestPlanFailureReasonUsesModelName(t *testing.T) {
	got := planFailureReason("codex", "Codex CLI not installed; run codex login; gpt-6-astra")
	if !strings.Contains(got, "Codex runtime") {
		t.Fatalf("failure reason missing model name: %q", got)
	}
	if strings.Contains(got, "Codex CLI") || strings.Contains(got, config.CodexModel) {
		t.Fatalf("failure reason leaked adapter text or model id: %q", got)
	}
	claude := planFailureReason("claude", config.ClaudeModel+" failed in Claude CLI")
	if claude != "claude failed in Claude runtime" {
		t.Fatalf("model-facing normalization did not use public claude name: %q", claude)
	}
}

func TestFormatPlanResult_SingleParsed(t *testing.T) {
	res := &PlanRunResult{Results: []PlanCLIResult{
		{CLI: "codex", Model: config.GPT56SolModel, Parsed: &PlanOutput{Summary: "s", Rating: 8}},
	}}
	out := FormatPlanResult(res, "/tmp/plan.md")
	if !strings.Contains(out, "═══ RIVAL PLAN REVIEW ═══") || !strings.Contains(out, "Rating: 8/10") {
		t.Errorf("single-parsed render wrong:\n%s", out)
	}
	// Single-CLI must NOT use the multi header.
	if strings.Contains(out, "RIVAL PLAN REVIEW (") {
		t.Errorf("single result should use the single-CLI header:\n%s", out)
	}
}

func TestFormatPlanResult_SingleParseFailReturnsRaw(t *testing.T) {
	res := &PlanRunResult{Results: []PlanCLIResult{
		{CLI: "codex", Model: config.GPT56SolModel, Parsed: nil, Raw: "Codex raw output"},
	}}
	out := FormatPlanResult(res, "/tmp/plan.md")
	if out != "Sol runtime raw output" {
		t.Errorf("parse-fail single result must preserve raw content with a model-facing label, got:\n%s", out)
	}
}

func TestFormatPlanResult_MultiBlocksAndSkipped(t *testing.T) {
	res := &PlanRunResult{
		Results: []PlanCLIResult{
			{CLI: "codex", Model: config.GPT56SolModel, Parsed: &PlanOutput{Summary: "cx", Rating: 6}},
			{CLI: "claude", Model: config.ClaudeModel, Parsed: nil, Raw: "Claude raw dump"},
		},
		Skipped: []SkippedCLI{{CLI: "opencode", Model: config.KimiModel, Reason: "n/a"}},
	}
	out := FormatPlanResult(res, "/tmp/plan.md")
	if !strings.Contains(out, "RIVAL PLAN REVIEW ("+config.SolLabel+" + "+config.ClaudeLabel+")") {
		t.Errorf("multi header missing engines:\n%s", out)
	}
	if !strings.Contains(out, "── "+config.SolLabel+" ──") {
		t.Errorf("sol block header missing:\n%s", out)
	}
	if !strings.Contains(out, "── "+config.ClaudeLabel+" ──") {
		t.Errorf("claude block header missing:\n%s", out)
	}
	// Claude block had no parsed output → raw fallback shown.
	if !strings.Contains(out, "Claude runtime raw dump") {
		t.Errorf("claude raw fallback missing:\n%s", out)
	}
	if !strings.Contains(out, "Skipped: kimi-k3 — n/a") {
		t.Errorf("skipped line missing:\n%s", out)
	}
	if strings.Contains(strings.ToLower(out), "codex") {
		t.Errorf("plan output must use model names, not adapter names:\n%s", out)
	}
}

func TestAssemblePlanResults_ErrUsesReason(t *testing.T) {
	// A timeout-style failure carries a Reason that must surface in Skipped,
	// instead of the bare error text.
	batch := []planCLIRun{
		{CLI: "claude", Err: errString("context deadline exceeded"), Reason: config.ClaudeModel + " run timeout after 30m (RIVAL_RUN_TIMEOUT) — model did not finish", ExitCode: -1},
		{CLI: "codex", Model: config.GPT56SolModel, Raw: realPlanJSON, ExitCode: 0},
	}
	res, err := assemblePlanResults(batch, nil)
	if err != nil {
		t.Fatalf("assemblePlanResults: %v", err)
	}
	if len(res.Skipped) != 1 || !strings.Contains(res.Skipped[0].Reason, "RIVAL_RUN_TIMEOUT") {
		t.Fatalf("want claude skipped with RIVAL_RUN_TIMEOUT reason, got %+v", res.Skipped)
	}
}

func TestRunPlanCLI_RestoresPlanMode(t *testing.T) {
	// Isolate the sessions dir (SessionDirPath uses $HOME) so this test never
	// writes into the user's real ~/.rival/sessions.
	t.Setenv("HOME", t.TempDir())

	// The claude executor overwrites sess.Mode to the transport ("native"); the
	// terminal session must be recorded as a plan session regardless.
	sess, err := session.NewQueued("claude", "plan", config.ClaudeModel, "high", t.TempDir(), "p", "/tmp/plan.md", "g")
	if err != nil {
		t.Fatal(err)
	}
	if err := sess.MarkRunning(); err != nil {
		t.Fatal(err)
	}
	ex := planExecutor{
		preflight: func(string) error { return nil },
		run: func(_ context.Context, s *session.Session, _, _, _, _ string) (string, int, error) {
			s.Mode = "native" // simulate the Claude executor clobbering mode
			return realPlanJSON, 0, nil
		},
	}
	out := runPlanCLI(context.Background(), ex, sess, "claude", "p", t.TempDir(), "plan")
	if out.ExitCode != 0 {
		t.Fatalf("run failed: %+v", out)
	}
	if sess.Mode != "plan" {
		t.Fatalf("sess.Mode = %q, want plan (restored after claude transport overwrote it)", sess.Mode)
	}
}

func TestRunPlanReviewResolvesPerModelEfforts(t *testing.T) {
	tests := []struct {
		name       string
		configYAML string
		override   string
		clis       []string
		want       map[string]string
	}{
		{
			name: "codex native fallback",
			clis: []string{"codex"},
			want: map[string]string{"codex": "xhigh"},
		},
		{
			name:       "codex configured effort",
			configYAML: "efforts:\n  codex: low\n",
			clis:       []string{"codex"},
			want:       map[string]string{"codex": "low"},
		},
		{
			name: "codex uses its xhigh pin",
			clis: []string{"codex"},
			want: map[string]string{"codex": "xhigh"},
		},
		{
			name: "claude alone uses its medium pin",
			clis: []string{"claude"},
			want: map[string]string{"claude": "medium"},
		},
		{
			name: "paired native plan keeps each pin",
			clis: []string{"codex", "claude"},
			want: map[string]string{"codex": "xhigh", "claude": "medium"},
		},
		{
			name:       "configured defaults resolve independently",
			configYAML: "efforts:\n  codex: low\n  claude: ultra\n",
			clis:       []string{"codex", "claude"},
			want:       map[string]string{"codex": "low", "claude": "ultra"},
		},
		{
			name:       "explicit override wins for every model",
			configYAML: "efforts:\n  codex: low\n  claude: medium\n",
			override:   "ultra",
			clis:       []string{"codex", "claude"},
			want:       map[string]string{"codex": "ultra", "claude": "ultra"},
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			loadPlanTestConfig(t, tc.configYAML)

			type observation struct {
				cli           string
				sessionEffort string
				runEffort     string
			}
			observed := make(chan observation, len(tc.clis))
			ex := planExecutor{
				preflight: func(string) error { return nil },
				run: func(_ context.Context, sess *session.Session, cli, _, effort, _ string) (string, int, error) {
					observed <- observation{cli: cli, sessionEffort: sess.Effort, runEffort: effort}
					return realPlanJSON, 0, nil
				},
			}

			_, err := runPlanReview(
				context.Background(),
				ex,
				"/tmp/plan.md",
				tc.override,
				t.TempDir(),
				"efforts",
				true,
				tc.clis,
			)
			if err != nil {
				t.Fatalf("runPlanReview: %v", err)
			}

			for range tc.clis {
				got := <-observed
				want := tc.want[got.cli]
				if got.sessionEffort != want {
					t.Errorf("%s session effort = %q, want %q", got.cli, got.sessionEffort, want)
				}
				if got.runEffort != got.sessionEffort {
					t.Errorf("%s executor effort = %q, want session effort %q", got.cli, got.runEffort, got.sessionEffort)
				}
			}
		})
	}
}

func TestRunPlanReviewPreservesRequestedOrderWhenClaudeFinishesFirst(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	claudeDone := make(chan struct{})
	ex := planExecutor{
		preflight: func(string) error { return nil },
		run: func(_ context.Context, _ *session.Session, cli, _, _, _ string) (string, int, error) {
			if cli == "claude" {
				close(claudeDone)
			} else {
				<-claudeDone
			}
			return realPlanJSON, 0, nil
		},
	}
	result, err := runPlanReview(context.Background(), ex, "/tmp/plan.md", "ultra", t.TempDir(), "ordered", true, []string{"codex", "claude"})
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Results) != 2 || result.Results[0].CLI != "codex" || result.Results[1].CLI != "claude" {
		t.Fatalf("plan results lost requested order: %+v", result.Results)
	}
}

func loadPlanTestConfig(t *testing.T, contents string) {
	t.Helper()

	home := t.TempDir()
	// Restore the process-global config only after t.Setenv has restored HOME.
	t.Cleanup(config.LoadUserConfig)
	t.Setenv("HOME", home)

	if contents != "" {
		dir := filepath.Join(home, ".rival")
		if err := os.MkdirAll(dir, 0o700); err != nil {
			t.Fatalf("create config dir: %v", err)
		}
		if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(contents), 0o600); err != nil {
			t.Fatalf("write config: %v", err)
		}
	}

	config.LoadUserConfig()
	if err := config.UserConfigError(); err != nil {
		t.Fatalf("load config: %v", err)
	}
}

// errString is a tiny error type so tests can build a planCLIRun.Err without fmt.
type errString string

func (e errString) Error() string { return string(e) }

// Antislop calls runDocReview directly with its own prompt and a high
// fallback effort (config override still wins); the target must land as the
// session's review scope. An empty fallback keeps plan semantics — covered by
// TestRunPlanReviewResolvesPerModelEfforts's lone-claude low case.
func TestRunDocReviewAppliesFallbackEffortAndTarget(t *testing.T) {
	loadPlanTestConfig(t, "")

	type observation struct {
		effort string
		scope  string
		prompt string
	}
	observed := make(chan observation, 1)
	ex := planExecutor{
		preflight: func(string) error { return nil },
		run: func(_ context.Context, sess *session.Session, _, prompt, effort, _ string) (string, int, error) {
			observed <- observation{effort: effort, scope: sess.ReviewScope, prompt: prompt}
			return realPlanJSON, 0, nil
		},
	}

	_, err := runDocReview(context.Background(), ex, "antislop", "ANTISLOP PROMPT", "src/api/", "", config.DefaultAntislopEffort, t.TempDir(), "doc", true, []string{"claude"})
	if err != nil {
		t.Fatalf("runDocReview: %v", err)
	}
	got := <-observed
	if got.effort != "medium" {
		t.Errorf("single-claude effort = %q, want the medium pin", got.effort)
	}
	if got.scope != "src/api/" {
		t.Errorf("session review scope = %q, want the antislop target", got.scope)
	}
	if got.prompt != "ANTISLOP PROMPT" {
		t.Errorf("prompt = %q, want the caller-built prompt passed through", got.prompt)
	}
}

// Antislop runs must carry their own session mode. They previously reused
// "plan", so every dashboard labelled them plan reviews.
func TestRunDocReviewRecordsTheRequestedMode(t *testing.T) {
	loadPlanTestConfig(t, "")

	observed := make(chan string, 1)
	ex := planExecutor{
		preflight: func(string) error { return nil },
		run: func(_ context.Context, sess *session.Session, _, _, _, _ string) (string, int, error) {
			observed <- sess.Mode
			return realPlanJSON, 0, nil
		},
	}

	_, err := runDocReview(context.Background(), ex, "antislop", "PROMPT", "src/", "", "xhigh", t.TempDir(), "mode", true, []string{"claude"})
	if err != nil {
		t.Fatalf("runDocReview: %v", err)
	}
	if got := <-observed; got != "antislop" {
		t.Errorf("session mode = %q, want antislop", got)
	}
}

// The plan wrapper must keep its own mode after the mode parameter lands.
func TestRunPlanReviewStillRecordsPlanMode(t *testing.T) {
	loadPlanTestConfig(t, "")

	observed := make(chan string, 1)
	ex := planExecutor{
		preflight: func(string) error { return nil },
		run: func(_ context.Context, sess *session.Session, _, _, _, _ string) (string, int, error) {
			observed <- sess.Mode
			return realPlanJSON, 0, nil
		},
	}

	_, err := runPlanReview(context.Background(), ex, "/tmp/plan.md", "", t.TempDir(), "mode", true, []string{"claude"})
	if err != nil {
		t.Fatalf("runPlanReview: %v", err)
	}
	if got := <-observed; got != "plan" {
		t.Errorf("session mode = %q, want plan", got)
	}
}

func TestAntislopCodexEffortReachesRuntime(t *testing.T) {
	for _, tt := range []struct{ name, config, override, want string }{
		{"default", "", "", "high"},
		{"configured", "efforts:\n  codex: medium\n", "", "medium"},
		{"explicit", "efforts:\n  codex: medium\n", "xhigh", "xhigh"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			loadPlanTestConfig(t, tt.config)
			observed := make(chan string, 1)
			ex := planExecutor{
				preflight: func(string) error { return nil },
				run: func(_ context.Context, sess *session.Session, _, _, effort, _ string) (string, int, error) {
					if sess.Effort != effort {
						return "", 1, errString("session/runtime effort mismatch")
					}
					observed <- effort
					return realPlanJSON, 0, nil
				},
			}
			_, err := runDocReview(context.Background(), ex, session.ModeAntislop, "review", "src/", tt.override, config.DefaultAntislopEffort, t.TempDir(), "effort", true, []string{"codex"})
			if err != nil {
				t.Fatal(err)
			}
			if got := <-observed; got != tt.want {
				t.Fatalf("runtime effort = %q, want %q", got, tt.want)
			}
		})
	}
}
