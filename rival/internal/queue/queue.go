// Package queue coordinates bounded review executions across independent rival
// processes via ticket files in ~/.rival/queue/ guarded by flock. No daemon:
// each process scans, reaps dead tickets, and promotes itself when a slot is
// free, all inside a single flock critical section. Queue ordering depends
// only on ticket files; the liveness check may additionally read the PIDs of
// the sessions a ticket references (a SIGKILL'd rival can leave a provider
// CLI child running — the slot stays held until that child dies).
//
// Liveness is PID + process-start-time based (see internal/procinfo): a ticket
// records the owner's start time, and a recycled PID — belonging to a different
// process with a different start time — is correctly treated as dead. On a
// platform where start time is unreadable, the check degrades to a bare
// kill(pid,0) existence test.
package queue

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/procinfo"
	"github.com/google/uuid"
	"github.com/rs/zerolog/log"
)

// ErrQueueTimeout is returned by WaitForSlot when no slot frees up in time.
var ErrQueueTimeout = errors.New("queue timeout")

// staleFileAge is how old an unparseable .json or leftover .tmp must be
// before scanners delete it (younger ones may be writes in flight).
const staleFileAge = 60 * time.Second

// Manager coordinates one process's place in the queue. All fields are
// injectable for tests; use New for production values.
type Manager struct {
	Dir           string
	MaxConcurrent int
	PollInterval  time.Duration
	Timeout       time.Duration // 0 = wait forever
	Now           func() time.Time
	// SessionLive reports whether a session is "running" with a live PID.
	SessionLive func(sessionID string) bool

	ticket *Ticket // own ticket after Enqueue
}

// New returns a Manager with production settings from config and env.
func New() *Manager {
	return &Manager{
		Dir:           config.QueueDirPath(),
		MaxConcurrent: config.MaxConcurrent(),
		PollInterval:  config.QueuePollInterval,
		Timeout:       config.QueueTimeout(),
		Now:           time.Now,
		SessionLive:   sessionLive,
	}
}

// Enqueue creates this process's ticket at the tail of the queue.
func (m *Manager) Enqueue(groupID string, sessionIDs []string, mode, workdir string) (*Ticket, error) {
	if err := os.MkdirAll(m.Dir, 0700); err != nil {
		return nil, fmt.Errorf("create queue dir: %w", err)
	}
	now := m.now()
	pid := os.Getpid()
	pidStart, _ := procinfo.StartNanos(pid)
	t := &Ticket{
		ID:         uuid.New().String(),
		GroupID:    groupID,
		SessionIDs: sessionIDs,
		Mode:       mode,
		PID:        pid,
		PIDStart:   pidStart,
		State:      StateWaiting,
		CreatedAt:  now,
		WorkDir:    workdir,
	}
	t.file = ticketFilename(now, t.PID, t.ID)
	if err := writeTicket(m.Dir, t); err != nil {
		return nil, err
	}
	m.ticket = t
	// Return a copy: WaitForSlot mutates the internal ticket (promotion,
	// self-heal) and callers must not share that memory.
	cp := *t
	return &cp, nil
}

// WaitForSlot blocks until this process's ticket is promoted to running.
// onPosition fires (outside the lock) whenever the 1-based position among
// waiting tickets changes; running is the count of held slots at that moment.
// Returns ctx.Err() on cancellation and ErrQueueTimeout past Timeout.
func (m *Manager) WaitForSlot(ctx context.Context, onPosition func(pos, total, running int)) error {
	if m.ticket == nil {
		return errors.New("WaitForSlot called before Enqueue")
	}
	var deadline time.Time
	if m.Timeout > 0 {
		deadline = m.now().Add(m.Timeout)
	}
	lastPos := -1
	for {
		var promoted, healed bool
		var pos, total, running int
		err := m.withLock(func() error {
			waiting, runningCount := m.scanLocked()
			running = runningCount
			idx := -1
			for i, t := range waiting {
				if t.ID == m.ticket.ID {
					idx = i
					break
				}
			}
			if idx == -1 {
				// Own ticket vanished (manual rm, queue clear) — self-heal by
				// re-creating at the tail. Position is recomputed next loop.
				now := m.now()
				m.ticket.CreatedAt = now
				m.ticket.file = ticketFilename(now, m.ticket.PID, m.ticket.ID)
				healed = true
				log.Warn().Str("ticket", m.ticket.ID).Msg("queue ticket vanished, re-creating at tail")
				return writeTicket(m.Dir, m.ticket)
			}
			pos, total = idx+1, len(waiting)
			if idx < m.MaxConcurrent-runningCount {
				now := m.now()
				m.ticket.State = StateRunning
				m.ticket.StartedAt = &now
				promoted = true
				return writeTicket(m.Dir, m.ticket)
			}
			return nil
		})
		if err != nil {
			return err
		}
		if promoted {
			return nil
		}
		if !healed && pos != lastPos && onPosition != nil {
			onPosition(pos, total, running)
			lastPos = pos
		}
		if !deadline.IsZero() && m.now().After(deadline) {
			return fmt.Errorf("%w after %s", ErrQueueTimeout, m.Timeout)
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(m.PollInterval):
		}
	}
}

// Release removes this process's ticket, freeing its slot. Idempotent.
func (m *Manager) Release() {
	if m.ticket == nil {
		return
	}
	if err := os.Remove(filepath.Join(m.Dir, m.ticket.file)); err != nil && !os.IsNotExist(err) {
		log.Warn().Err(err).Str("ticket", m.ticket.ID).Msg("failed to remove queue ticket")
	}
	m.ticket = nil
}

// Entry is a ticket with its computed queue position (0 for running tickets).
type Entry struct {
	Ticket   Ticket
	Position int
}

// List returns all live tickets: running ones first, then waiting in FIFO
// order with 1-based positions. Dead tickets are reaped as a side effect.
func (m *Manager) List() ([]Entry, error) {
	var entries []Entry
	err := m.withLock(func() error {
		waiting, _ := m.scanLocked()
		running := m.readRunningLocked()
		for _, t := range running {
			entries = append(entries, Entry{Ticket: *t, Position: 0})
		}
		for i, t := range waiting {
			entries = append(entries, Entry{Ticket: *t, Position: i + 1})
		}
		return nil
	})
	return entries, err
}

// ReapDead removes tickets whose rival process and referenced sessions are
// all dead. Called from PersistentPreRun so stale tickets clear even when no
// waiter is polling.
func (m *Manager) ReapDead() {
	if _, err := os.Stat(m.Dir); err != nil {
		return // no queue dir yet — nothing to reap
	}
	_ = m.withLock(func() error {
		m.scanLocked()
		return nil
	})
}

// Clear removes dead tickets, or all tickets when force is set (live waiters
// self-heal back to the tail). Returns the number removed.
func (m *Manager) Clear(force bool) (int, error) {
	removed := 0
	err := m.withLock(func() error {
		files, err := m.ticketFiles()
		if err != nil {
			return err
		}
		for _, f := range files {
			t, ok := m.readTicket(f)
			if !ok {
				continue
			}
			if force || !m.ticketAlive(t) {
				if err := os.Remove(filepath.Join(m.Dir, f)); err == nil {
					removed++
				}
			}
		}
		return nil
	})
	return removed, err
}

// --- internals ---

func (m *Manager) now() time.Time {
	if m.Now != nil {
		return m.Now()
	}
	return time.Now()
}

// withLock runs fn while holding an exclusive flock on <dir>/.lock. The lock
// file is never deleted (delete+recreate would split the lock across inodes).
func (m *Manager) withLock(fn func() error) error {
	if err := os.MkdirAll(m.Dir, 0700); err != nil {
		return fmt.Errorf("create queue dir: %w", err)
	}
	f, err := os.OpenFile(filepath.Join(m.Dir, ".lock"), os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return fmt.Errorf("open queue lock: %w", err)
	}
	defer func() { _ = f.Close() }()
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX); err != nil {
		return fmt.Errorf("flock queue: %w", err)
	}
	defer func() { _ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN) }()
	return fn()
}

// scanLocked reads all tickets, reaps dead ones and stale garbage, and
// returns the waiting tickets in FIFO order plus the live running count.
// Must be called with the flock held.
func (m *Manager) scanLocked() (waiting []*Ticket, running int) {
	files, err := m.ticketFiles()
	if err != nil {
		return nil, 0
	}
	for _, f := range files {
		t, ok := m.readTicket(f)
		if !ok {
			continue
		}
		if !m.ticketAlive(t) {
			log.Info().Str("ticket", t.ID).Int("pid", t.PID).Str("state", t.State).Msg("reaping dead queue ticket")
			_ = os.Remove(filepath.Join(m.Dir, f))
			continue
		}
		switch t.State {
		case StateRunning:
			running++
		default:
			waiting = append(waiting, t)
		}
	}
	return waiting, running
}

// readRunningLocked returns live running tickets (no reaping; assumes
// scanLocked already ran in this critical section).
func (m *Manager) readRunningLocked() []*Ticket {
	files, err := m.ticketFiles()
	if err != nil {
		return nil
	}
	var out []*Ticket
	for _, f := range files {
		if t, ok := m.readTicket(f); ok && t.State == StateRunning {
			out = append(out, t)
		}
	}
	return out
}

// ticketFiles returns ticket filenames sorted FIFO (by name = by unixnano
// prefix) and cleans up stale .tmp leftovers.
func (m *Manager) ticketFiles() ([]string, error) {
	entries, err := os.ReadDir(m.Dir)
	if err != nil {
		return nil, err
	}
	var files []string
	for _, e := range entries {
		name := e.Name()
		switch {
		case strings.HasSuffix(name, ".json"):
			files = append(files, name)
		case strings.HasSuffix(name, ".tmp"):
			m.removeIfStale(name, e)
		}
	}
	sort.Strings(files)
	return files, nil
}

// readTicket parses a ticket file. Unparseable files are skipped, and deleted
// only once old enough that they cannot be a write in flight.
func (m *Manager) readTicket(name string) (*Ticket, bool) {
	path := filepath.Join(m.Dir, name)
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, false
	}
	var t Ticket
	if err := json.Unmarshal(data, &t); err != nil || t.ID == "" {
		if info, statErr := os.Stat(path); statErr == nil && m.now().Sub(info.ModTime()) > staleFileAge {
			log.Warn().Str("file", name).Msg("removing stale unparseable queue file")
			_ = os.Remove(path)
		}
		return nil, false
	}
	t.file = name
	return &t, true
}

func (m *Manager) removeIfStale(name string, e os.DirEntry) {
	info, err := e.Info()
	if err == nil && m.now().Sub(info.ModTime()) > staleFileAge {
		_ = os.Remove(filepath.Join(m.Dir, name))
	}
}

// ticketAlive: the rival process is alive, or one of the ticket's sessions is
// still running with a live PID (e.g. surviving provider CLI child after the
// rival process was SIGKILL'd — that child still consumes rate limit, so the
// slot must stay held).
func (m *Manager) ticketAlive(t *Ticket) bool {
	if t.PID > 0 && procinfo.Alive(t.PID, t.PIDStart) {
		return true
	}
	if m.SessionLive == nil {
		return false
	}
	for _, sid := range t.SessionIDs {
		if m.SessionLive(sid) {
			return true
		}
	}
	return false
}

// sessionLive is the production SessionLive: reads the session JSON directly
// (minimal struct — avoids importing the session package) and checks that it
// is running with a live PID (PID-reuse-guarded via the recorded start time).
func sessionLive(id string) bool {
	data, err := os.ReadFile(filepath.Join(config.SessionDirPath(), id+".json"))
	if err != nil {
		return false
	}
	var s struct {
		Status   string `json:"status"`
		PID      int    `json:"pid"`
		PIDStart int64  `json:"pid_start"`
	}
	if json.Unmarshal(data, &s) != nil {
		return false
	}
	return s.Status == "running" && procinfo.Alive(s.PID, s.PIDStart)
}
