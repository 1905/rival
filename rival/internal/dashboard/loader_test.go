package dashboard

import (
	"context"
	"time"

	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"
)

// loadingModel is a sized model that has not seen a SessionEvent yet.
func loadingModel(t *testing.T, width, height int) Model {
	t.Helper()
	m := New()
	t.Cleanup(m.cancel)
	return send(t, m, tea.WindowSizeMsg{Width: width, Height: height})
}

func assertFrameExact(t *testing.T, m Model, label string) {
	t.Helper()
	assertFrameWidth(t, m, label)
	if n := strings.Count(m.viewContent(), "\n") + 1; n != m.lay.Height {
		t.Fatalf("%s: frame has %d lines, want %d", label, n, m.lay.Height)
	}
}

func TestLoaderShowsBeforeFirstSnapshot(t *testing.T) {
	for _, h := range []int{16, 24, 40} {
		m := loadingModel(t, 100, h)
		v := ansi.Strip(m.viewContent())
		if !strings.Contains(v, "reading sessions") {
			t.Fatalf("height %d: no loader:\n%s", h, v)
		}
		if strings.Contains(v, "No sessions yet") {
			t.Fatalf("height %d: empty state shown while loading:\n%s", h, v)
		}
		for _, zero := range []string{"0 running", "0 queued", "✓ 0", "✗ 0", "0 sessions", "ALL 0"} {
			if strings.Contains(v, zero) {
				t.Fatalf("height %d: %q shown while loading:\n%s", h, zero, v)
			}
		}
		for _, dots := range []string{"… running", "… queued", "✓ …", "… sessions", "ALL …"} {
			if !strings.Contains(v, dots) {
				t.Fatalf("height %d: want %q while loading:\n%s", h, dots, v)
			}
		}
	}
}

func TestLoadProgressUpdatesTheBar(t *testing.T) {
	m := loadingModel(t, 100, 30)
	before := m.viewContent()
	m = send(t, m, LoadProgress{Done: 1240, Total: 2999})
	v := ansi.Strip(m.viewContent())
	if !strings.Contains(v, "reading sessions 1240/2999") {
		t.Fatalf("progress text missing:\n%s", v)
	}
	if !strings.Contains(v, "█") || !strings.Contains(v, "░") {
		t.Fatalf("a 41%% bar should be part filled, part empty:\n%s", v)
	}
	if m.viewContent() == before {
		t.Fatal("frame did not change after LoadProgress")
	}
}

func TestFirstSessionEventRemovesTheLoader(t *testing.T) {
	m := loadingModel(t, 100, 30)
	m = send(t, m, LoadProgress{Done: 100, Total: 4}, SessionEvent{Sessions: listFixture()})
	v := ansi.Strip(m.viewContent())
	if strings.Contains(v, "reading sessions") || strings.Contains(v, "…") {
		t.Fatalf("loader survived the first snapshot:\n%s", v)
	}
	if !strings.Contains(v, "4 sessions") {
		t.Fatalf("header counts missing after load:\n%s", v)
	}
	// A late progress message must not bring the loader back.
	m = send(t, m, LoadProgress{Done: 1, Total: 4})
	if strings.Contains(ansi.Strip(m.viewContent()), "reading sessions") {
		t.Fatal("late LoadProgress revived the loader")
	}
}

func TestEmptySnapshotEndsLoadingAtOnce(t *testing.T) {
	m := send(t, loadingModel(t, 100, 30), SessionEvent{})
	v := ansi.Strip(m.viewContent())
	if strings.Contains(v, "reading sessions") || !strings.Contains(v, "No sessions yet") {
		t.Fatalf("want the empty state after an empty snapshot:\n%s", v)
	}
	if !strings.Contains(v, "0 sessions") {
		t.Fatalf("want real zero counts after load:\n%s", v)
	}
}

func TestLoaderFrameIsExact(t *testing.T) {
	for _, width := range []int{60, 90, 120, 200} {
		for _, height := range []int{16, 24, 29, 30, 50} {
			m := loadingModel(t, width, height)
			assertFrameExact(t, m, "loader, no progress")
			m = send(t, m, LoadProgress{Done: 2999, Total: 2999})
			assertFrameExact(t, m, "loader, full bar")
			m = send(t, m, press("?"))
			assertFrameExact(t, m, "loader, full help")
		}
	}
}

// WatchSessions closes the progress channel once the initial scan is sent, so
// the loader's wait chain ends, also when there are no sessions at all.
func TestWatchSessionsClosesProgressOnEmptyDir(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	events := make(chan SessionEvent, 1)
	progress := make(chan LoadProgress, 1)
	if err := WatchSessions(ctx, events, progress); err != nil {
		t.Fatal(err)
	}
	select {
	case ev := <-events:
		if len(ev.Sessions) != 0 {
			t.Fatalf("got %d sessions, want 0", len(ev.Sessions))
		}
	case <-time.After(2 * time.Second):
		t.Fatal("no initial SessionEvent")
	}
	if _, ok := <-progress; ok {
		t.Fatal("progress not closed after the initial scan")
	}
	if msg := waitForProgress(progress)(); msg != nil {
		t.Fatalf("waitForProgress on a closed channel = %v, want nil", msg)
	}
}
