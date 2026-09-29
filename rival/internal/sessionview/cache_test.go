package sessionview

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// writeSession writes a minimal session JSON file and returns its path.
func writeSession(t *testing.T, dir, id, status string, start time.Time) string {
	t.Helper()
	path := filepath.Join(dir, id+".json")
	body := fmt.Sprintf(`{
  "id": %q,
  "cli": "codex",
  "model": "gpt-5.6-sol",
  "mode": "review",
  "status": %q,
  "effort": "high",
  "work_dir": "/tmp",
  "prompt": "a long stored prompt that summaries drop",
  "start_time": %q
}`, id, status, start.Format(time.RFC3339Nano))
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
	return path
}

func TestCacheLoadsAllSessionsNewestFirst(t *testing.T) {
	dir := t.TempDir()
	now := time.Now()
	writeSession(t, dir, "older", "completed", now.Add(-time.Hour))
	writeSession(t, dir, "newer", "completed", now)

	sessions, rev := New(dir).Load()
	if len(sessions) != 2 {
		t.Fatalf("got %d sessions, want 2", len(sessions))
	}
	if sessions[0].ID != "newer" {
		t.Errorf("first session = %q, want newer", sessions[0].ID)
	}
	if rev == 0 {
		t.Error("revision stayed 0 after the initial load")
	}
}

func TestCacheRevisionOnlyMovesOnChange(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "a", "completed", time.Now())
	cache := New(dir)

	_, first := cache.Load()
	_, second := cache.Load()
	if first != second {
		t.Fatalf("revision moved without a file change: %d then %d", first, second)
	}

	// A rewrite with new content and a new mtime must bump the revision.
	time.Sleep(10 * time.Millisecond)
	writeSession(t, dir, "a", "failed", time.Now())
	sessions, third := cache.Load()
	if third == second {
		t.Error("revision did not move after the file changed")
	}
	if len(sessions) != 1 || sessions[0].Status != "failed" {
		t.Errorf("cache did not reparse the changed file: %+v", sessions)
	}
}

func TestCacheDropsDeletedFiles(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "a", "completed", time.Now())
	path := writeSession(t, dir, "b", "completed", time.Now())
	cache := New(dir)

	if sessions, _ := cache.Load(); len(sessions) != 2 {
		t.Fatalf("got %d sessions, want 2", len(sessions))
	}
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	sessions, _ := cache.Load()
	if len(sessions) != 1 || sessions[0].ID != "a" {
		t.Errorf("deleted session still cached: %+v", sessions)
	}
	if cache.Get("b") != nil {
		t.Error("Get returned a deleted session")
	}
}

func TestCacheSkipsUnparsableFileAndTempFiles(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "good", "completed", time.Now())
	if err := os.WriteFile(filepath.Join(dir, "broken.json"), []byte("{not json"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "partial.json.tmp"), []byte("{}"), 0o644); err != nil {
		t.Fatal(err)
	}

	sessions, _ := New(dir).Load()
	if len(sessions) != 1 || sessions[0].ID != "good" {
		t.Errorf("got %+v, want only the good session", sessions)
	}
}

func TestCacheOnAbsentDirectoryReturnsNil(t *testing.T) {
	sessions, rev := New(filepath.Join(t.TempDir(), "missing")).Load()
	if sessions != nil {
		t.Errorf("got %+v, want nil for an absent directory", sessions)
	}
	if rev != 0 {
		t.Errorf("revision = %d, want 0", rev)
	}
}

func TestCacheGetReturnsCachedSession(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "wanted", "running", time.Now())
	cache := New(dir)
	cache.Load()

	got := cache.Get("wanted")
	if got == nil || got.ID != "wanted" {
		t.Fatalf("Get returned %+v, want the wanted session", got)
	}
	if cache.Get("absent") != nil {
		t.Error("Get returned a session that was never cached")
	}
}

func TestCacheProgressIsThrottledAndReachesTotal(t *testing.T) {
	dir := t.TempDir()
	const n = 250
	for i := 0; i < n; i++ {
		writeSession(t, dir, fmt.Sprintf("s%03d", i), "completed", time.Now())
	}
	var calls [][2]int
	sessions, _ := New(dir).LoadWithProgress(func(done, total int) {
		calls = append(calls, [2]int{done, total})
	})
	if len(sessions) != n {
		t.Fatalf("got %d sessions, want %d", len(sessions), n)
	}
	want := [][2]int{{100, n}, {200, n}, {n, n}}
	if fmt.Sprint(calls) != fmt.Sprint(want) {
		t.Fatalf("progress calls = %v, want %v", calls, want)
	}
}

func TestCacheProgressSilentOnEmptyDir(t *testing.T) {
	called := false
	New(t.TempDir()).LoadWithProgress(func(int, int) { called = true })
	if called {
		t.Fatal("progress called with no session files")
	}
}
