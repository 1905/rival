package session

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/procinfo"
	"github.com/google/uuid"
)

// pidStartNanos returns the start time of pid, or 0 if unavailable.
func pidStartNanos(pid int) int64 {
	n, _ := procinfo.StartNanos(pid)
	return n
}

// Session modes. The dashboards label a run by its mode, so a new run type
// needs its own value here rather than reusing an existing one.
const (
	ModePlan     = "plan"
	ModeAntislop = "antislop"
	ModeSecurity = "security"
)

// IsTaskMode reports whether mode names a task rather than a transport.
// A task mode identifies the run in both dashboards, so a runtime must not
// overwrite it with a transport name such as "native" or "docker".
func IsTaskMode(mode string) bool {
	return mode == ModePlan || mode == ModeAntislop || mode == ModeSecurity
}

type Session struct {
	ID            string     `json:"id"`
	GroupID       string     `json:"group_id,omitempty"`
	CLI           string     `json:"cli"`
	Mode          string     `json:"mode"`
	Model         string     `json:"model"`
	Effort        string     `json:"effort"`
	ReviewScope   string     `json:"review_scope,omitempty"`
	Prompt        string     `json:"prompt,omitempty"`
	PromptPreview string     `json:"prompt_preview,omitempty"`
	PromptHash    string     `json:"prompt_hash,omitempty"`
	Status        string     `json:"status"`
	StartTime     time.Time  `json:"start_time"`
	QueuedAt      *time.Time `json:"queued_at,omitempty"`
	QueuePosition int        `json:"queue_position,omitempty"`
	EndTime       *time.Time `json:"end_time,omitempty"`
	ExitCode      *int       `json:"exit_code,omitempty"`
	Duration      string     `json:"duration,omitempty"`
	WorkDir       string     `json:"work_dir"`
	LogFile       string     `json:"log_file"`
	OutputBytes   int64      `json:"output_bytes"`
	OutputLines   int        `json:"output_lines"`
	ErrorMsg      string     `json:"error,omitempty"`
	Account       string     `json:"account,omitempty"`
	PID           int        `json:"pid"`
	PIDStart      int64      `json:"pid_start,omitempty"` // start time of PID (Unix ns); guards against PID reuse
	// OwnerPID is the rival process driving this session. PID is overwritten
	// with the provider child's PID once the subprocess starts, so without
	// this field the reaper cannot tell "provider exited, rival is about to
	// write the final status" (a normal end-of-run window) from "everything
	// is dead". Sessions written by older releases have 0 here.
	OwnerPID      int   `json:"owner_pid,omitempty"`
	OwnerPIDStart int64 `json:"owner_pid_start,omitempty"`
}

// NewQueued creates a session in "queued" state — visible in the TUI while
// the process waits for a queue slot. Call MarkRunning when the slot is acquired.
func NewQueued(cli, mode, model, effort, workdir, prompt, reviewScope, groupID string) (*Session, error) {
	return create(cli, mode, model, effort, workdir, prompt, reviewScope, groupID, "queued")
}

func create(cli, mode, model, effort, workdir, prompt, reviewScope, groupID, status string) (*Session, error) {
	dir := config.SessionDirPath()
	if err := os.MkdirAll(dir, 0700); err != nil {
		return nil, fmt.Errorf("create session dir: %w", err)
	}

	id := uuid.New().String()
	logFile := filepath.Join(dir, id+".log")

	preview := prompt
	if len(preview) > config.PromptPreviewLen {
		preview = preview[:config.PromptPreviewLen]
	}

	hash := fmt.Sprintf("%x", sha256.Sum256([]byte(prompt)))

	now := time.Now()
	s := &Session{
		ID:            id,
		GroupID:       groupID,
		CLI:           cli,
		Mode:          mode,
		Model:         model,
		Effort:        effort,
		ReviewScope:   reviewScope,
		Prompt:        prompt,
		PromptPreview: preview,
		PromptHash:    hash,
		Status:        status,
		StartTime:     now,
		WorkDir:       workdir,
		LogFile:       logFile,
		// The rival process's own PID until the subprocess starts — keeps the
		// reaper accurate during a potentially long queue wait (PID 0 would be
		// treated as dead and the session insta-failed). PIDStart guards the PID
		// against reuse after this process dies.
		PID:      os.Getpid(),
		PIDStart: pidStartNanos(os.Getpid()),
		// The owner never changes: as long as this rival process is alive it
		// is responsible for finalizing the session, and the reaper must not.
		OwnerPID:      os.Getpid(),
		OwnerPIDStart: pidStartNanos(os.Getpid()),
	}
	if status == "queued" {
		s.QueuedAt = &now
	}

	if err := s.Save(); err != nil {
		return nil, err
	}
	return s, nil
}

// MarkRunning transitions a queued session to running. StartTime is reset so
// Duration measures runtime, not queue wait; QueuedAt preserves the wait.
func (s *Session) MarkRunning() error {
	s.Status = "running"
	s.StartTime = time.Now()
	s.QueuePosition = 0
	return s.Save()
}

// SetQueuePosition updates the displayed queue position. No-op (and no file
// write / fsnotify event) when unchanged.
func (s *Session) SetQueuePosition(pos int) error {
	if s.QueuePosition == pos {
		return nil
	}
	s.QueuePosition = pos
	return s.Save()
}

// Save writes the session JSON atomically: a unique <id>.json.tmp-* file,
// then rename. The unique name matters because several processes save the same
// session (the owning rival, the TUI stop, the reaper, the Mac app); with one
// shared temp name, two concurrent writers could interleave into a partial
// file and rename it into place. The temp name never ends in ".json", so
// readers that glob or suffix-match "*.json" skip it.
func (s *Session) Save() error {
	dir := config.SessionDirPath()
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal session: %w", err)
	}

	f, err := os.CreateTemp(dir, s.ID+".json.tmp-*") // mode 0600
	if err != nil {
		return fmt.Errorf("create session tmp: %w", err)
	}
	tmp := f.Name()
	_, werr := f.Write(data)
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	if werr != nil {
		_ = os.Remove(tmp)
		return fmt.Errorf("write session tmp: %w", werr)
	}
	if err := os.Rename(tmp, filepath.Join(dir, s.ID+".json")); err != nil {
		_ = os.Remove(tmp) // clean up orphaned temp file
		return fmt.Errorf("rename session: %w", err)
	}
	return nil
}

// Complete marks the session as completed.
func (s *Session) Complete(exitCode int, outputBytes int64, outputLines int) error {
	now := time.Now()
	s.Status = "completed"
	s.ExitCode = &exitCode
	s.EndTime = &now
	s.Duration = now.Sub(s.StartTime).Round(time.Second).String()
	s.OutputBytes = outputBytes
	s.OutputLines = outputLines
	return s.Save()
}

// Fail marks the session as failed.
func (s *Session) Fail(exitCode int, errMsg string) error {
	now := time.Now()
	s.Status = "failed"
	s.ExitCode = &exitCode
	s.EndTime = &now
	s.Duration = now.Sub(s.StartTime).Round(time.Second).String()
	s.ErrorMsg = errMsg
	return s.Save()
}

// isTempName reports whether name is a Save temp file: the legacy
// "<id>.json.tmp" or the unique "<id>.json.tmp-*".
func isTempName(name string) bool {
	return strings.Contains(name, ".json.tmp")
}

// IsSessionFile reports whether name (a base name) is a finished session
// record, not a temp file or anything else in the sessions dir.
func IsSessionFile(name string) bool {
	return strings.HasSuffix(name, ".json") && !isTempName(name)
}

// LoadAll reads and returns all sessions, sorted newest first.
func LoadAll() []*Session {
	dir := config.SessionDirPath()
	matches, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil {
		return nil
	}

	var sessions []*Session
	for _, path := range matches {
		if isTempName(filepath.Base(path)) {
			continue
		}
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var s Session
		if err := json.Unmarshal(data, &s); err != nil {
			continue
		}
		sessions = append(sessions, &s)
	}

	sort.Slice(sessions, func(i, j int) bool {
		return sessions[i].StartTime.After(sessions[j].StartTime)
	})

	return sessions
}

// Load reads one complete session record by id.
func Load(id string) (*Session, error) {
	data, err := os.ReadFile(filepath.Join(config.SessionDirPath(), id+".json"))
	if err != nil {
		return nil, err
	}
	var s Session
	if err := json.Unmarshal(data, &s); err != nil {
		return nil, err
	}
	return &s, nil
}

// SortGroupMembers restores the order in which a grouped run requested its
// models. QueuedAt is the creation timestamp and, unlike StartTime, is not
// reset as members are promoted to running. The deterministic fallbacks keep
// legacy sessions stable when they do not have queue metadata.
func SortGroupMembers(sessions []*Session) {
	sort.SliceStable(sessions, func(i, j int) bool {
		a, b := sessions[i], sessions[j]
		if rankA, rankB := groupModeRank(a.Mode), groupModeRank(b.Mode); rankA != rankB {
			return rankA < rankB
		}
		if a.QueuedAt != nil && b.QueuedAt != nil && !a.QueuedAt.Equal(*b.QueuedAt) {
			return a.QueuedAt.Before(*b.QueuedAt)
		}
		if rankA, rankB := groupModelRank(a), groupModelRank(b); rankA != rankB {
			return rankA < rankB
		}
		if !a.StartTime.Equal(b.StartTime) {
			return a.StartTime.Before(b.StartTime)
		}
		return a.ID < b.ID
	})
}

func groupModeRank(mode string) int {
	if mode == "consilium" {
		return 1
	}
	return 0
}

func groupModelRank(s *Session) int {
	switch config.EngineLabel(s.CLI, s.Model) {
	case config.SolLabel: // read-compat: display of sessions recorded before Sol's removal
		return 0
	case "kimi-k3":
		return 1
	case config.ClaudeLabel:
		return 2
	case config.GrokLabel:
		return 3
	default:
		return 100
	}
}

// OpenLog opens the session log file for writing.
func (s *Session) OpenLog() (*os.File, error) {
	return os.OpenFile(s.LogFile, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
}
