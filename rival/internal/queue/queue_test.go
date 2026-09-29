package queue

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/1905/rival/internal/procinfo"
)

// deadPID is far above any real PID on macOS/Linux defaults.
const deadPID = 1 << 24

// procStartOK reports whether process start time is readable on this platform.
func procStartOK() (int64, bool) {
	return procinfo.StartNanos(os.Getpid())
}

// liveSessions is a concurrency-safe stub for Manager.SessionLive.
type liveSessions struct {
	mu  sync.Mutex
	ids map[string]bool
}

func (l *liveSessions) set(id string, live bool) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.ids == nil {
		l.ids = map[string]bool{}
	}
	l.ids[id] = live
}

func (l *liveSessions) live(id string) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.ids[id]
}

func newTestManager(dir string, maxConcurrent int, live *liveSessions) *Manager {
	return &Manager{
		Dir:           dir,
		MaxConcurrent: maxConcurrent,
		PollInterval:  5 * time.Millisecond,
		Timeout:       2 * time.Second,
		SessionLive:   live.live,
	}
}

// writeRawTicket plants a ticket file directly, simulating another process.
func writeRawTicket(t *testing.T, dir string, nano int64, pid int, state string, sessionIDs []string) string {
	t.Helper()
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	id := fmt.Sprintf("raw-%d", nano)
	// Record the real start time for live PIDs so the reuse guard sees a match;
	// dead PIDs get 0 (irrelevant — they're not alive anyway).
	pidStart, _ := procinfo.StartNanos(pid)
	tk := &Ticket{
		ID:         id,
		SessionIDs: sessionIDs,
		Mode:       "test",
		PID:        pid,
		PIDStart:   pidStart,
		State:      state,
		CreatedAt:  time.Unix(0, nano),
		file:       ticketFilename(time.Unix(0, nano), pid, id),
	}
	if err := writeTicket(dir, tk); err != nil {
		t.Fatal(err)
	}
	return tk.file
}

func TestImmediatePromoteOnEmptyQueue(t *testing.T) {
	dir := t.TempDir()
	m := newTestManager(dir, 1, &liveSessions{})
	if _, err := m.Enqueue("", []string{"s1"}, "review", "/tmp"); err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	if err := m.WaitForSlot(context.Background(), nil); err != nil {
		t.Fatal(err)
	}
	if d := time.Since(start); d > 500*time.Millisecond {
		t.Fatalf("promotion on empty queue took %v, want immediate", d)
	}
	if m.ticket.State != StateRunning {
		t.Fatalf("state = %q, want running", m.ticket.State)
	}
}

func TestFIFOOrder(t *testing.T) {
	dir := t.TempDir()
	live := &liveSessions{}

	m1 := newTestManager(dir, 1, live)
	if _, err := m1.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	if err := m1.WaitForSlot(context.Background(), nil); err != nil {
		t.Fatal(err)
	}

	m2 := newTestManager(dir, 1, live)
	m3 := newTestManager(dir, 1, live)
	if _, err := m2.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	time.Sleep(time.Millisecond) // distinct unixnano prefixes
	if _, err := m3.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}

	order := make(chan int, 2)
	var wg sync.WaitGroup
	for i, m := range []*Manager{m2, m3} {
		wg.Add(1)
		go func(n int, mgr *Manager) {
			defer wg.Done()
			if err := mgr.WaitForSlot(context.Background(), nil); err != nil {
				t.Errorf("waiter %d: %v", n, err)
				return
			}
			order <- n
			mgr.Release()
		}(i+2, m)
	}

	select {
	case n := <-order:
		t.Fatalf("waiter %d promoted while slot held", n)
	case <-time.After(50 * time.Millisecond):
	}

	m1.Release()
	wg.Wait()
	close(order)
	var got []int
	for n := range order {
		got = append(got, n)
	}
	if len(got) != 2 || got[0] != 2 || got[1] != 3 {
		t.Fatalf("promotion order = %v, want [2 3]", got)
	}
}

func TestCapacityTwoPromotesBothInOneCycle(t *testing.T) {
	dir := t.TempDir()
	live := &liveSessions{}

	m1 := newTestManager(dir, 2, live)
	m2 := newTestManager(dir, 2, live)
	m3 := newTestManager(dir, 2, live)
	for _, m := range []*Manager{m1, m2, m3} {
		if _, err := m.Enqueue("", nil, "review", ""); err != nil {
			t.Fatal(err)
		}
		time.Sleep(time.Millisecond)
	}

	for i, m := range []*Manager{m1, m2} {
		if err := m.WaitForSlot(context.Background(), nil); err != nil {
			t.Fatalf("waiter %d: %v", i+1, err)
		}
	}

	done := make(chan error, 1)
	go func() { done <- m3.WaitForSlot(context.Background(), nil) }()
	select {
	case <-done:
		t.Fatal("third waiter promoted with both slots held")
	case <-time.After(50 * time.Millisecond):
	}

	m1.Release()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestDeadTicketReapedThenPromote(t *testing.T) {
	dir := t.TempDir()
	writeRawTicket(t, dir, 1, deadPID, StateRunning, nil)
	writeRawTicket(t, dir, 2, deadPID, StateWaiting, nil)

	m := newTestManager(dir, 1, &liveSessions{})
	if _, err := m.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	if err := m.WaitForSlot(context.Background(), nil); err != nil {
		t.Fatal(err)
	}
	entries, err := m.List()
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 {
		t.Fatalf("entries = %d, want 1 (dead tickets reaped)", len(entries))
	}
}

func TestSIGKILLSurvivorHoldsSlot(t *testing.T) {
	dir := t.TempDir()
	live := &liveSessions{}
	live.set("survivor", true)
	// Running ticket: rival PID dead, but its session's provider child lives.
	writeRawTicket(t, dir, 1, deadPID, StateRunning, []string{"survivor"})

	m := newTestManager(dir, 1, live)
	if _, err := m.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- m.WaitForSlot(context.Background(), nil) }()
	select {
	case <-done:
		t.Fatal("promoted while surviving child holds the slot")
	case <-time.After(100 * time.Millisecond):
	}

	live.set("survivor", false) // child exited
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestUnparseableFileTolerance(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	fresh := filepath.Join(dir, "0000000001-1-garbage.json")
	if err := os.WriteFile(fresh, []byte("{not json"), 0600); err != nil {
		t.Fatal(err)
	}
	old := filepath.Join(dir, "0000000002-1-old.json")
	if err := os.WriteFile(old, []byte("{not json"), 0600); err != nil {
		t.Fatal(err)
	}
	past := time.Now().Add(-2 * staleFileAge)
	if err := os.Chtimes(old, past, past); err != nil {
		t.Fatal(err)
	}

	m := newTestManager(dir, 1, &liveSessions{})
	if _, err := m.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	if err := m.WaitForSlot(context.Background(), nil); err != nil {
		t.Fatalf("garbage files must not block promotion: %v", err)
	}
	if _, err := os.Stat(fresh); err != nil {
		t.Fatal("fresh unparseable file was deleted; could be a write in flight")
	}
	if _, err := os.Stat(old); !os.IsNotExist(err) {
		t.Fatal("stale unparseable file was not cleaned up")
	}
}

func TestCtxCancelWhileWaiting(t *testing.T) {
	dir := t.TempDir()
	writeRawTicket(t, dir, 1, os.Getpid(), StateRunning, nil)

	m := newTestManager(dir, 1, &liveSessions{})
	if _, err := m.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- m.WaitForSlot(ctx, nil) }()
	time.Sleep(20 * time.Millisecond)
	cancel()
	if err := <-done; !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
}

func TestQueueTimeout(t *testing.T) {
	dir := t.TempDir()
	writeRawTicket(t, dir, 1, os.Getpid(), StateRunning, nil)

	m := newTestManager(dir, 1, &liveSessions{})
	m.Timeout = 50 * time.Millisecond
	if _, err := m.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	if err := m.WaitForSlot(context.Background(), nil); !errors.Is(err, ErrQueueTimeout) {
		t.Fatalf("err = %v, want ErrQueueTimeout", err)
	}
}

// TestPIDReuseGuardReapsRecycledHolder: a running ticket whose PID is live but
// whose recorded start time does NOT match the live process (i.e. the PID was
// recycled by an unrelated process) must be reaped, freeing the slot.
func TestPIDReuseGuardReapsRecycledHolder(t *testing.T) {
	if _, ok := procStartOK(); !ok {
		t.Skip("process start time unsupported on this platform")
	}
	dir := t.TempDir()
	// Plant a running ticket with OUR live pid but a bogus start time.
	id := "recycled"
	tk := &Ticket{
		ID:        id,
		Mode:      "test",
		PID:       os.Getpid(),
		PIDStart:  1, // deliberately wrong — simulates a recycled PID
		State:     StateRunning,
		CreatedAt: time.Unix(0, 1),
		file:      ticketFilename(time.Unix(0, 1), os.Getpid(), id),
	}
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := writeTicket(dir, tk); err != nil {
		t.Fatal(err)
	}

	m := newTestManager(dir, 1, &liveSessions{})
	if _, err := m.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}
	// The recycled-PID holder must be reaped → we promote immediately.
	if err := m.WaitForSlot(context.Background(), nil); err != nil {
		t.Fatalf("recycled-PID ticket should have been reaped: %v", err)
	}
}

func TestSelfHealAfterTicketRemoved(t *testing.T) {
	dir := t.TempDir()
	holder := writeRawTicket(t, dir, 1, os.Getpid(), StateRunning, nil)

	m := newTestManager(dir, 1, &liveSessions{})
	tk, err := m.Enqueue("", nil, "review", "")
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- m.WaitForSlot(context.Background(), nil) }()
	time.Sleep(20 * time.Millisecond)

	if err := os.Remove(filepath.Join(dir, tk.file)); err != nil {
		t.Fatal(err)
	}
	time.Sleep(50 * time.Millisecond) // let the waiter self-heal
	if err := os.Remove(filepath.Join(dir, holder)); err != nil {
		t.Fatal(err)
	}
	if err := <-done; err != nil {
		t.Fatalf("waiter did not recover from removed ticket: %v", err)
	}
}

func TestPositionCallback(t *testing.T) {
	dir := t.TempDir()
	holder := writeRawTicket(t, dir, 1, os.Getpid(), StateRunning, nil)
	ahead := writeRawTicket(t, dir, 2, os.Getpid(), StateWaiting, nil)

	m := newTestManager(dir, 1, &liveSessions{})
	if _, err := m.Enqueue("", nil, "review", ""); err != nil {
		t.Fatal(err)
	}

	var mu sync.Mutex
	var positions []int
	done := make(chan error, 1)
	go func() {
		done <- m.WaitForSlot(context.Background(), func(pos, total, running int) {
			mu.Lock()
			positions = append(positions, pos)
			mu.Unlock()
		})
	}()
	time.Sleep(30 * time.Millisecond)
	if err := os.Remove(filepath.Join(dir, ahead)); err != nil {
		t.Fatal(err)
	}
	time.Sleep(30 * time.Millisecond)
	if err := os.Remove(filepath.Join(dir, holder)); err != nil && !os.IsNotExist(err) {
		t.Fatal(err)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(positions) < 2 || positions[0] != 2 || positions[len(positions)-1] != 1 {
		t.Fatalf("positions = %v, want [2 ... 1]", positions)
	}
}

func TestClearForceAndDeadOnly(t *testing.T) {
	dir := t.TempDir()
	writeRawTicket(t, dir, 1, deadPID, StateRunning, nil)
	liveFile := writeRawTicket(t, dir, 2, os.Getpid(), StateWaiting, nil)

	m := newTestManager(dir, 1, &liveSessions{})
	removed, err := m.Clear(false)
	if err != nil {
		t.Fatal(err)
	}
	if removed != 1 {
		t.Fatalf("dead-only clear removed %d, want 1", removed)
	}
	if _, err := os.Stat(filepath.Join(dir, liveFile)); err != nil {
		t.Fatal("dead-only clear removed a live ticket")
	}

	removed, err = m.Clear(true)
	if err != nil {
		t.Fatal(err)
	}
	if removed != 1 {
		t.Fatalf("force clear removed %d, want 1", removed)
	}
}
