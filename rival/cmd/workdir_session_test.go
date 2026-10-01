package cmd

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/1905/rival/internal/session"
)

// TestCommandRelativeWorkdirStoresAbsolutePath proves a relative --workdir is
// resolved once at the command entry: the provider gets an absolute dir (so
// codex -C / opencode --dir / grok --cwd cannot re-apply it on top of
// cmd.Dir) and the session records the absolute path, not ".".
func TestCommandRelativeWorkdirStoresAbsolutePath(t *testing.T) {
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, "v3"), 0o700); err != nil {
		t.Fatal(err)
	}
	t.Chdir(root)
	// Resolve symlinks (macOS /var -> /private/var) the way os.Getwd sees it.
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	want := filepath.Join(cwd, "v3")

	f := &fakeRun{log: "answer\n"}
	if _, err := runCommandWith(t, f, "explain the auth flow", "v3/"); err != nil {
		t.Fatalf("run: %v", err)
	}
	if f.workdir != want {
		t.Errorf("provider workdir = %q, want %q", f.workdir, want)
	}
	sessions := session.LoadAll()
	if len(sessions) != 1 {
		t.Fatalf("got %d sessions, want 1", len(sessions))
	}
	if got := sessions[0].WorkDir; got != want {
		t.Errorf("session work_dir = %q, want %q", got, want)
	}
}
