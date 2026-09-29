package session

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"sort"

	"github.com/1905/rival/internal/config"
)

const summaryEdgeBytes = 64 << 10

// LoadAllSummaries returns the same newest-first session index as LoadAll
// without retaining embedded prompts. Large legacy files are read only at
// their edges, where MarshalIndent places the metadata around the prompt line.
func LoadAllSummaries() []*Session {
	dir := config.SessionDirPath()
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}

	sessions := make([]*Session, 0, len(entries))
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !IsSessionFile(name) {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		s, err := LoadSummaryFile(filepath.Join(dir, name), info.Size())
		if err != nil {
			continue
		}
		sessions = append(sessions, s)
	}
	sort.Slice(sessions, func(i, j int) bool {
		return sessions[i].StartTime.After(sessions[j].StartTime)
	})
	return sessions
}

// LoadSummaryFile reads one session without retaining its full prompt.
func LoadSummaryFile(path string, size int64) (*Session, error) {
	if size <= 2*summaryEdgeBytes {
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, err
		}
		var s Session
		if err := json.Unmarshal(data, &s); err != nil {
			return nil, err
		}
		s.Prompt = ""
		return &s, nil
	}

	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer func() { _ = f.Close() }()

	prefix := make([]byte, summaryEdgeBytes)
	n, err := io.ReadFull(f, prefix)
	if err != nil && !errors.Is(err, io.ErrUnexpectedEOF) {
		return nil, err
	}
	prefix = prefix[:n]

	if _, err := f.Seek(-summaryEdgeBytes, io.SeekEnd); err != nil {
		return nil, err
	}
	suffix := make([]byte, summaryEdgeBytes)
	n, err = io.ReadFull(f, suffix)
	if err != nil && !errors.Is(err, io.ErrUnexpectedEOF) {
		return nil, err
	}
	suffix = suffix[:n]

	rawFields := make(map[string]json.RawMessage)
	collectSummaryFields(rawFields, prefix)
	collectSummaryFields(rawFields, suffix)
	if len(rawFields) == 0 {
		return nil, errors.New("session metadata not found")
	}

	data, err := json.Marshal(rawFields)
	if err != nil {
		return nil, err
	}
	var s Session
	if err := json.Unmarshal(data, &s); err != nil {
		return nil, err
	}
	return &s, nil
}

var summaryFields = map[string]bool{
	"id":              true,
	"group_id":        true,
	"cli":             true,
	"mode":            true,
	"model":           true,
	"effort":          true,
	"review_scope":    true,
	"prompt_preview":  true,
	"prompt_hash":     true,
	"status":          true,
	"start_time":      true,
	"queued_at":       true,
	"queue_position":  true,
	"end_time":        true,
	"exit_code":       true,
	"duration":        true,
	"work_dir":        true,
	"log_file":        true,
	"output_bytes":    true,
	"output_lines":    true,
	"error":           true,
	"account":         true,
	"pid":             true,
	"pid_start":       true,
	"owner_pid":       true,
	"owner_pid_start": true,
}

func collectSummaryFields(dst map[string]json.RawMessage, data []byte) {
	for _, line := range bytes.Split(data, []byte{'\n'}) {
		line = bytes.TrimSpace(line)
		if len(line) < 4 || line[0] != '"' {
			continue
		}
		colon := bytes.IndexByte(line, ':')
		if colon < 2 {
			continue
		}
		var key string
		if err := json.Unmarshal(line[:colon], &key); err != nil || !summaryFields[key] {
			continue
		}
		value := bytes.TrimSpace(line[colon+1:])
		value = bytes.TrimSuffix(value, []byte{','})
		if !json.Valid(value) {
			continue
		}
		dst[key] = append(json.RawMessage(nil), value...)
	}
}
