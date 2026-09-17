package executor

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/1F47E/rival/internal/session"
)

// TestSafeEnv_BlocksOpencodePermission proves a reviewed repo's .env cannot inject
// OPENCODE_PERMISSION (or other OPENCODE_* / proxy vars) into a child CLI — the
// read-only opencode reviewer sandbox must not be configurable by the code under
// review. Unrelated vars still pass through.
func TestSafeEnv_BlocksOpencodePermission(t *testing.T) {
	t.Setenv("OPENCODE_PERMISSION", `{"bash":"allow"}`)
	t.Setenv("OPENCODE_CONFIG", "/tmp/evil.json")
	t.Setenv("HTTPS_PROXY", "http://evil:8080")
	t.Setenv("RIVAL_SAFEENV_KEEP", "keepme")

	env := safeEnv()
	joined := strings.Join(env, "\n")

	for _, blocked := range []string{"OPENCODE_PERMISSION=", "OPENCODE_CONFIG=", "HTTPS_PROXY="} {
		if strings.Contains(joined, blocked) {
			t.Errorf("safeEnv leaked a blocked var: %s", blocked)
		}
	}
	if !strings.Contains(joined, "RIVAL_SAFEENV_KEEP=keepme") {
		t.Errorf("safeEnv dropped an unrelated var that should pass through")
	}
}

// TestSafeEnv_BlocksGrokRuntimeVars proves a reviewed repo's .env cannot
// reconfigure the authenticated grok runtime (proxy/base URL, GROK_HOME, auth
// helpers) or slip in XAI_API_KEY fallback auth. PATH still survives so the
// child can be found at all.
func TestSafeEnv_BlocksGrokRuntimeVars(t *testing.T) {
	t.Setenv("GROK_ANYTHING", "x")
	t.Setenv("XAI_API_KEY", "x")

	env := safeEnv()
	joined := strings.Join(env, "\n")

	for _, blocked := range []string{"GROK_ANYTHING=", "XAI_API_KEY="} {
		if strings.Contains(joined, blocked) {
			t.Errorf("safeEnv leaked a blocked var: %s", blocked)
		}
	}
	if !strings.Contains(joined, "PATH=") {
		t.Errorf("safeEnv dropped PATH")
	}
}

func TestSafeEnvKeepsReviewerInItsOwnRepository(t *testing.T) {
	for _, key := range []string{"GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY", "GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS"} {
		t.Setenv(key, "caller-repository")
	}
	t.Setenv("GIT_SSH_COMMAND", "ssh -p 2222")
	for _, item := range safeEnv() {
		if strings.HasSuffix(item, "=caller-repository") {
			t.Fatalf("reviewer inherited a caller repository override: %s", item)
		}
	}
	if !strings.Contains(strings.Join(safeEnv(), "\n"), "GIT_SSH_COMMAND=ssh -p 2222") {
		t.Fatal("removed host SSH authentication settings")
	}
}

// TestRunSubprocess_ContextTimeoutKillsChild proves RIVAL_RUN_TIMEOUT's
// mechanism: a context deadline kills a hung child promptly instead of waiting
// for it to finish. Uses /bin/sleep so no provider quota is touched.
func TestRunSubprocess_ContextTimeoutKillsChild(t *testing.T) {
	// Keep all session file writes inside a temp HOME.
	t.Setenv("HOME", t.TempDir())

	sess, err := session.NewQueued("test", "raw", "none", "low", t.TempDir(), "", "", "")
	if err != nil {
		t.Fatalf("create session: %v", err)
	}
	if err := sess.MarkRunning(); err != nil {
		t.Fatalf("mark running: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	start := time.Now()
	// sleep would run 5s; the 100ms deadline must cut it short.
	result, runErr := RunSubprocess(ctx, sess, "sleep", []string{"5"}, nil, "", nil)
	elapsed := time.Since(start)

	if elapsed > 2*time.Second {
		t.Fatalf("expected child killed promptly, but it ran %v", elapsed)
	}
	if ctx.Err() != context.DeadlineExceeded {
		t.Errorf("expected DeadlineExceeded, got %v", ctx.Err())
	}
	// A killed child surfaces as either a non-nil error or a nonzero exit code;
	// the contract we care about is "did not run to completion".
	if runErr == nil && result != nil && result.ExitCode == 0 {
		t.Errorf("expected nonzero/killed result, got clean exit 0")
	}
}
