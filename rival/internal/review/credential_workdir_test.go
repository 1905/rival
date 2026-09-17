package review

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestMRReviewKeepsProjectCredentialsAndSnapshotWorkdir(t *testing.T) {
	root := t.TempDir()
	source, snapshot, bin := filepath.Join(root, "source"), filepath.Join(root, "snapshot"), filepath.Join(root, "bin")
	for _, dir := range []string{source, snapshot, bin} {
		if err := os.Mkdir(dir, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	files := map[string]string{
		filepath.Join(source, ".env"):   "MOONSHOT_API_KEY=caller-test-key\n",
		filepath.Join(snapshot, ".env"): "MOONSHOT_API_KEY=untrusted-snapshot-key\n",
		filepath.Join(bin, "opencode"): `#!/bin/sh
cat >> "$MR_TEST_PROMPTS"
printf '%s\n' "$PWD" "$OPENCODE_CONFIG_CONTENT" "$OPENCODE_PERMISSION" >> "$MR_TEST_RUNS"
printf '%s\n' '{"summary":"Fixture review complete","findings":[],"recommendation":{"status":"approve","summary":"Fixture verdict"}}'
`,
	}
	for path, content := range files {
		if err := os.WriteFile(path, []byte(content), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("HOME", root)
	t.Setenv("MOONSHOT_API_KEY", "")
	t.Setenv("KIMI_API", "")
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("MR_TEST_PROMPTS", filepath.Join(root, "prompts"))
	t.Setenv("MR_TEST_RUNS", filepath.Join(root, "runs"))
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	ctx = WithCredentialWorkdir(ctx, source)
	const scope = "Pinned MR patch: +changed line from source head"
	result, err := RunMegaReviewWithModels(ctx, scope, "", snapshot, "fixture-mr", true, []string{"k3"})
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Inputs) != 1 || len(result.Skipped) != 0 || result.Output.Recommendation.Status != "approve" {
		t.Fatalf("incomplete reviewer/judge pipeline: %+v", result)
	}
	runs, err := os.ReadFile(filepath.Join(root, "runs"))
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"caller-test-key", `"bash":"deny"`, filepath.Base(snapshot)} {
		if strings.Count(string(runs), want) != 2 {
			t.Fatalf("reviewer and judge must both use %q: %s", want, runs)
		}
	}
	if strings.Contains(string(runs), "untrusted-snapshot-key") {
		t.Fatal("used credentials from the reviewed MR")
	}
	prompts, err := os.ReadFile(filepath.Join(root, "prompts"))
	if err != nil || strings.Count(string(prompts), scope) != 2 {
		t.Fatalf("reviewer and judge did not receive the pinned patch: %v", err)
	}
}
