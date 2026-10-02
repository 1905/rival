package executor

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
)

// dropEnv entries ending in "_" are prefix drops — "AWS_" must catch the whole
// credential family (SESSION_TOKEN, PROFILE, WEB_IDENTITY_TOKEN_FILE, …), not
// just the three canonical names; exact entries must not over-match.
func TestDropMatchesPrefixAndExact(t *testing.T) {
	cases := []struct {
		kv   string
		want bool
	}{
		{"AWS_ACCESS_KEY_ID=x", true},
		{"AWS_WEB_IDENTITY_TOKEN_FILE=/path", true},
		{"AWS_PROFILE=default", true},
		{"OPENAI_API_KEY=sk-x", true},
		{"OPENAI_API_KEY_BACKUP=sk-x", false}, // exact entry, different name
		{"AWSOME_VAR=x", false},               // prefix requires the underscore
		{"PATH=/usr/bin", false},
	}
	for _, tc := range cases {
		if got := dropMatches(tc.kv, kimiDropEnv); got != tc.want {
			t.Errorf("dropMatches(%q) = %v, want %v", tc.kv, got, tc.want)
		}
	}
}

// Project-loaded Moonshot variables never pass through raw to a child; the key
// reaches OpenCode only through OPENCODE_CONFIG_CONTENT.
func TestMoonshotEnvPrefixesAreBlocked(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "sk-secret")
	t.Setenv("KIMI_API", "sk-secret")
	for _, kv := range safeEnv() {
		if strings.HasPrefix(kv, "KIMI_") || strings.HasPrefix(kv, "MOONSHOT_") {
			t.Errorf("safeEnv leaked %q", kv)
		}
	}
}

// Review mode keeps the zero-value read-only reviewer defaults; raw mode goes
// full-auto with the credential strip. Both carry the Moonshot key.
func TestKimiRunOptsByMode(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "test-key")

	review := kimiRunOpts("review", t.TempDir())
	if review.Permission != "" {
		t.Errorf("review mode must keep the read-only default profile, got %q", review.Permission)
	}
	if len(review.DropEnv) != 0 {
		t.Errorf("review mode needs no extra drops (bash is denied), got %v", review.DropEnv)
	}
	if review.APIKey != "test-key" {
		t.Errorf("review APIKey = %q, want test-key", review.APIKey)
	}

	raw := kimiRunOpts("raw", t.TempDir())
	if raw.Permission != opencodeFullAutoPermission {
		t.Errorf("raw mode must run full-auto, got %q", raw.Permission)
	}
	if len(raw.DropEnv) != len(kimiDropEnv) {
		t.Errorf("raw mode must strip credentials, got %v", raw.DropEnv)
	}
}

// K3 must receive the Moonshot key through its provider config.
func TestMoonshotModelUsesKimiKey(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "sk-moonshot")

	env := strings.Join(opencodeRunEnvWith("sess-1", mustK3Entry(t), "", OpencodeRunOpts{}), "\n")
	if !strings.Contains(env, "sk-moonshot") {
		t.Errorf("moonshot model env missing API key: %s", env)
	}
	if !strings.Contains(env, `"moonshotai"`) {
		t.Errorf("provider config must target the built-in moonshotai provider: %s", env)
	}
}

// The megareview k3 path has no explicit APIKey override, so the moonshot
// fallback must run the .env walk-up from the workdir — a review launched
// from a project subdirectory has to find the repo-root Moonshot key.
func TestMoonshotFallbackWalksUpFromWorkdir(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "")
	t.Setenv("KIMI_API", "")
	// Preflight only looks the CLI up; the fake fails loudly if it is ever run.
	bin := t.TempDir()
	t.Setenv("HOME", t.TempDir())
	t.Setenv("PATH", bin)
	if err := os.WriteFile(filepath.Join(bin, "opencode"), []byte("#!/bin/sh\necho 'fake opencode must not run' >&2\nexit 97\n"), 0700); err != nil {
		t.Fatal(err)
	}
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, ".env"), []byte("MOONSHOT_API_KEY=sk-walkup\n"), 0600); err != nil {
		t.Fatal(err)
	}
	sub := filepath.Join(root, "sub", "dir")
	if err := os.MkdirAll(sub, 0700); err != nil {
		t.Fatal(err)
	}
	env := strings.Join(opencodeRunEnvWith("sess-4", mustK3Entry(t), sub, OpencodeRunOpts{}), "\n")
	if !strings.Contains(env, "sk-walkup") {
		t.Errorf("moonshot fallback did not walk up to the workdir .env: %s", env)
	}
	if err := OpencodePreflightModel(config.KimiModel, sub); err != nil {
		t.Errorf("preflight should find the walked-up key: %v", err)
	}
}

func TestKimiRawEnvUsesFullAutoPermission(t *testing.T) {
	t.Setenv("MOONSHOT_API_KEY", "test-key")
	env := strings.Join(opencodeRunEnvWith("sess-3", mustK3Entry(t), "", kimiRunOpts("raw", t.TempDir())), "\n")
	if !strings.Contains(env, "OPENCODE_PERMISSION="+opencodeFullAutoPermission) {
		t.Errorf("raw env missing full-auto permission: %s", env)
	}
}
