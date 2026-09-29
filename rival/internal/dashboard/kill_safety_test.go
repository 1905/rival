package dashboard

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/1905/rival/internal/session"
)

// The TUI list is built from summaries, which never carry the full prompt.
// Killing a session must reload the stored record before it writes, or the
// save destroys the prompt on disk.
func TestKillReloadsBeforeFail(t *testing.T) {
	// SessionDirPath derives from the home directory, so pointing HOME at a
	// temp dir keeps this test away from the real session store.
	home := t.TempDir()
	t.Setenv("HOME", home)
	dir := filepath.Join(home, ".rival", "sessions")

	const storedPrompt = "the full prompt that a summary drops on the floor"
	full, err := session.NewQueued("codex", "review", "gpt-5.6-sol", "high", t.TempDir(), storedPrompt, "src/", "")
	if err != nil {
		t.Fatalf("NewQueued: %v", err)
	}
	if err := full.MarkRunning(); err != nil {
		t.Fatalf("MarkRunning: %v", err)
	}

	path := filepath.Join(dir, full.ID+".json")
	info, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat session file: %v", err)
	}

	// This is what the list holds after the shared cache lands: a summary.
	summary, err := session.LoadSummaryFile(path, info.Size())
	if err != nil {
		t.Fatalf("LoadSummaryFile: %v", err)
	}
	if summary.Prompt != "" {
		t.Fatalf("test premise broken: the summary carries a prompt (%q)", summary.Prompt)
	}

	// The kill path under test.
	failSessionForKill(summary, 137, "killed by user")

	reloaded, err := session.Load(full.ID)
	if err != nil {
		t.Fatalf("reload session: %v", err)
	}
	if reloaded.Prompt != storedPrompt {
		t.Errorf("prompt after kill = %q, want it preserved as %q", reloaded.Prompt, storedPrompt)
	}
	if reloaded.Status != "failed" {
		t.Errorf("status after kill = %q, want failed", reloaded.Status)
	}
	if reloaded.ExitCode == nil || *reloaded.ExitCode != 137 {
		t.Errorf("exit code after kill = %v, want 137", reloaded.ExitCode)
	}
}
