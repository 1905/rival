package session

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/1905/rival/internal/config"
)

// Codex finding 4: concurrent writers of one session (owner, TUI stop, reaper,
// the Mac app) must never share a temp file. With the old fixed
// "<id>.json.tmp", one writer's rename moved the other's half-written temp
// into place, or left it nothing to rename.
func TestSaveConcurrentWritersNeverShareATempFile(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	s, err := NewQueued("codex", "review", "gpt-5.5", "high", t.TempDir(), "prompt", "", "")
	if err != nil {
		t.Fatal(err)
	}
	dir := config.SessionDirPath()

	var wg sync.WaitGroup
	errs := make(chan error, 400)
	for w := 0; w < 20; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			c := *s // each writer saves its own copy, like separate processes
			for i := 0; i < 20; i++ {
				if err := c.Save(); err != nil {
					errs <- err
				}
			}
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Fatalf("concurrent Save failed: %v", err)
	}

	if _, err := Load(s.ID); err != nil {
		t.Fatalf("record unreadable after concurrent saves: %v", err)
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if e.Name() != s.ID+".json" {
			t.Errorf("leftover file %s", e.Name())
		}
	}
}

// A temp file another writer holds open is never reused or renamed away, and
// no reader treats either temp form as a session.
func TestSaveLeavesForeignTempFilesAndReadersSkipThem(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	s, err := NewQueued("codex", "review", "gpt-5.5", "high", t.TempDir(), "prompt", "", "")
	if err != nil {
		t.Fatal(err)
	}
	dir := config.SessionDirPath()
	foreign := filepath.Join(dir, s.ID+".json.tmp")
	if err := os.WriteFile(foreign, []byte(`{"id":"tmp-legacy","status":"running"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	unique := filepath.Join(dir, "other.json.tmp-123456")
	if err := os.WriteFile(unique, []byte(`{"id":"tmp-unique","status":"running"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := s.Save(); err != nil {
		t.Fatal(err)
	}
	if got, err := os.ReadFile(foreign); err != nil || !strings.Contains(string(got), "tmp-legacy") {
		t.Fatalf("foreign temp file clobbered: %q, %v", got, err)
	}
	info, err := os.Stat(filepath.Join(dir, s.ID+".json"))
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Errorf("mode %v, want 0600", info.Mode().Perm())
	}

	for _, got := range LoadAll() {
		if got.ID != s.ID {
			t.Errorf("LoadAll read temp file as session %s", got.ID)
		}
	}
	for _, got := range LoadAllSummaries() {
		if got.ID != s.ID {
			t.Errorf("LoadAllSummaries read temp file as session %s", got.ID)
		}
	}
	for name, want := range map[string]bool{
		"a.json": true, "a.json.tmp": false, "a.json.tmp-123": false, "a.log": false,
	} {
		if IsSessionFile(name) != want {
			t.Errorf("IsSessionFile(%q) = %v, want %v", name, !want, want)
		}
	}
}
