package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/logfmt"
	"github.com/1F47E/rival/internal/session"
	"github.com/1F47E/rival/internal/sessionview"
)

func TestGroupSessions(t *testing.T) {
	tests := []struct {
		name        string
		sessions    []*session.Session
		wantGroups  int
		wantIsGroup []bool // per resulting group, in order
		wantCLI     []string
		wantKind    []string // "" for solo groups
	}{
		{
			name:       "empty",
			sessions:   nil,
			wantGroups: 0,
		},
		{
			name: "two solo sessions stay separate",
			sessions: []*session.Session{
				{ID: "a", CLI: "codex", Model: config.GPT56SolModel, Status: "completed"},
				{ID: "b", CLI: "opencode", Model: config.KimiModel, Status: "completed"},
			},
			wantGroups:  2,
			wantIsGroup: []bool{false, false},
			wantCLI:     []string{config.SolLabel, config.K3Label},
			wantKind:    []string{"", ""},
		},
		{
			name: "shared GroupID collapses into one mega row",
			sessions: []*session.Session{
				{ID: "a", GroupID: "g1", CLI: "codex", Mode: "megareview", Model: config.GPT56SolModel, Status: "completed"},
				{ID: "b", GroupID: "g1", CLI: "opencode", Mode: "megareview", Model: config.KimiModel, Status: "completed"},
			},
			wantGroups:  1,
			wantIsGroup: []bool{true},
			wantCLI:     []string{config.SolLabel + "+" + config.K3Label},
			wantKind:    []string{"megareview"},
		},
		{
			name: "plan group is labelled with concrete model names",
			sessions: []*session.Session{
				{ID: "a", GroupID: "gp", CLI: "codex", Mode: "plan", Model: config.GPT56SolModel, Status: "completed"},
				{ID: "b", GroupID: "gp", CLI: "claude", Mode: "plan", Model: config.ClaudeModel, Status: "completed"},
			},
			wantGroups:  1,
			wantIsGroup: []bool{true},
			wantCLI:     []string{config.SolLabel + "+" + config.ClaudeLabel},
			wantKind:    []string{"plan"},
		},
		{
			name: "mixed: one mega group + one solo",
			sessions: []*session.Session{
				{ID: "a", GroupID: "g1", CLI: "codex", Mode: "megareview", Model: config.GPT56SolModel, Status: "completed"},
				{ID: "b", GroupID: "g1", CLI: "opencode", Mode: "megareview", Model: config.KimiModel, Status: "completed"},
				{ID: "c", CLI: "claude", Model: config.ClaudeModel, Status: "running"},
			},
			wantGroups:  2,
			wantIsGroup: []bool{true, false},
			wantCLI:     []string{config.SolLabel + "+" + config.K3Label, config.ClaudeLabel},
			wantKind:    []string{"megareview", ""},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := groupSessions(tt.sessions)
			if len(got) != tt.wantGroups {
				t.Fatalf("groupSessions() returned %d groups, want %d", len(got), tt.wantGroups)
			}
			for i := range got {
				if got[i].IsGroup != tt.wantIsGroup[i] {
					t.Errorf("group %d IsGroup = %v, want %v", i, got[i].IsGroup, tt.wantIsGroup[i])
				}
				if got[i].CLI != tt.wantCLI[i] {
					t.Errorf("group %d CLI = %q, want %q", i, got[i].CLI, tt.wantCLI[i])
				}
				if tt.wantKind != nil && got[i].Kind != tt.wantKind[i] {
					t.Errorf("group %d Kind = %q, want %q", i, got[i].Kind, tt.wantKind[i])
				}
			}
		})
	}
}

func TestPairedPlanGroupUsesPublicModels(t *testing.T) {
	created := time.Now()
	later := created.Add(time.Millisecond)
	groups := groupSessions([]*session.Session{
		{ID: "b", GroupID: "paired", CLI: "claude", Mode: "plan", Model: config.ClaudeModel, Status: "completed", QueuedAt: &later},
		{ID: "a", GroupID: "paired", CLI: "codex", Mode: "plan", Model: config.GPT56SolModel, Status: "completed", QueuedAt: &created},
	})
	if len(groups) != 1 {
		t.Fatalf("paired plan group count = %d, want 1", len(groups))
	}
	group := groups[0]
	if group.ID != "paired" {
		t.Fatalf("paired plan stable id = %q, want group id", group.ID)
	}
	if group.Kind != "plan" || group.CLI != "sol+claude" || group.Models != "sol + claude" {
		t.Fatalf("paired plan group labels = kind %q cli %q models %q", group.Kind, group.CLI, group.Models)
	}
	if len(group.Sessions) != 2 || group.Sessions[0].CLI != "sol" || group.Sessions[0].Model != "sol" || group.Sessions[1].CLI != "claude" || group.Sessions[1].Model != "claude" {
		t.Fatalf("paired plan public sessions = %+v", group.Sessions)
	}
}

func TestSingletonPlanKeepsLogicalGroupIdentity(t *testing.T) {
	groups := groupSessions([]*session.Session{
		{ID: "claude", GroupID: "degraded-plan", CLI: "claude", Mode: "plan", Model: config.ClaudeModel, Status: "failed", ErrorMsg: "claude failed"},
	})
	if len(groups) != 1 {
		t.Fatalf("singleton plan group count = %d, want 1", len(groups))
	}
	group := groups[0]
	if !group.IsGroup || group.ID != "degraded-plan" || group.Kind != "plan" || group.CLI != "claude" || group.Sessions[0].ErrorMsg != "claude failed" {
		t.Fatalf("singleton plan group = %+v", group)
	}
}

func TestSelectedReviewGroupUsesPublicModels(t *testing.T) {
	created := time.Now()
	times := make([]time.Time, 2)
	for i := range times {
		times[i] = created.Add(time.Duration(i) * time.Millisecond)
	}
	sessions := []*session.Session{
		{ID: "b", GroupID: "review", CLI: "opencode", Mode: "megareview", Model: config.KimiModel, Status: "failed", ErrorMsg: "k3 failed", QueuedAt: &times[1]},
		{ID: "a", GroupID: "review", CLI: "codex", Mode: "megareview", Model: config.GPT56SolModel, Status: "completed", QueuedAt: &times[0]},
	}
	groups := groupSessions(sessions)
	if len(groups) != 1 {
		t.Fatalf("review group count = %d, want 1", len(groups))
	}
	group := groups[0]
	wantLabels := []string{config.SolLabel, config.K3Label}
	if group.Kind != "megareview" || group.CLI != strings.Join(wantLabels, "+") || group.Models != strings.Join(wantLabels, " + ") {
		t.Fatalf("review group labels = kind %q cli %q models %q", group.Kind, group.CLI, group.Models)
	}
	gotLabels := make([]string, 0, len(group.Sessions))
	for _, s := range group.Sessions {
		if s.CLI != s.Model {
			t.Fatalf("public session labels disagree: %+v", s)
		}
		gotLabels = append(gotLabels, s.Model)
	}
	if !slices.Equal(gotLabels, wantLabels) {
		t.Fatalf("public review sessions = %v, want %v", gotLabels, wantLabels)
	}
	if group.Sessions[1].ErrorMsg != "k3 failed" {
		t.Fatalf("non-primary failure missing from public group: %+v", group.Sessions[1])
	}
}

func TestGroupSessionsReportsMixedEffort(t *testing.T) {
	groups := groupSessions([]*session.Session{
		{ID: "a", GroupID: "review", CLI: "codex", Model: config.GPT56SolModel, Effort: "ultra"},
		{ID: "b", GroupID: "review", CLI: "opencode", Model: config.KimiModel, Effort: "low"},
	})
	if len(groups) != 1 || groups[0].Effort != "mixed" {
		t.Fatalf("group effort = %+v, want mixed", groups)
	}
	if groups[0].Sessions[0].Effort != "ultra" || groups[0].Sessions[1].Effort != "low" {
		t.Fatalf("member efforts = %+v, want ultra and low", groups[0].Sessions)
	}
}

func TestIndexIncludesCuratedModelIcons(t *testing.T) {
	data, err := indexHTML.ReadFile("templates/index.html")
	if err != nil {
		t.Fatal(err)
	}
	html := string(data)
	for _, label := range []string{config.SolLabel, config.K3Label, config.ClaudeLabel, config.GrokLabel} {
		if !strings.Contains(html, label+"': '") && !strings.Contains(html, label+": '") {
			t.Errorf("web dashboard has no icon mapping for %q", label)
		}
	}
	for _, behavior := range []string{
		"const id = g.id",
		"g => g.id === id",
		"s.mode === 'consilium' ? 'JUDGE' : 'REVIEW'",
		"if (s.error) logText += 'Error: '",
		"Waiting for a queue slot",
		"unavailable = true",
		"s.status === 'failed' || unavailable",
		"syncOpenDetail(selected)",
		"state.detailGroup",
		"member-status",
		"if (group.is_group && s.effort) facts.push('effort ' + s.effort)",
		"window.addEventListener('hashchange'",
		"byId('detail-drawer').scrollTop = 0",
		"fetchRun(id",
		"Session detail is no longer available",
	} {
		if !strings.Contains(html, behavior) {
			t.Errorf("web dashboard missing grouped-session behavior %q", behavior)
		}
	}
}

func TestGroupStatus(t *testing.T) {
	tests := []struct {
		name     string
		statuses []string
		want     string
	}{
		{"all completed", []string{"completed", "completed"}, "completed"},
		{"any running wins", []string{"completed", "running", "failed"}, "running"},
		{"failed over completed", []string{"completed", "failed"}, "failed"},
		{"single failed", []string{"failed"}, "failed"},
		{"queued over failed and completed", []string{"completed", "queued", "failed"}, "queued"},
		{"running over queued", []string{"queued", "running"}, "running"},
		{"single queued", []string{"queued"}, "queued"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var sessions []*session.Session
			for _, s := range tt.statuses {
				sessions = append(sessions, &session.Session{Status: s})
			}
			if got := sessionview.Status(sessions); got != tt.want {
				t.Errorf("sessionview.Status(%v) = %q, want %q", tt.statuses, got, tt.want)
			}
		})
	}
}

func TestGroupModels_Dedupes(t *testing.T) {
	sessions := []*session.Session{
		{Model: config.GPT56SolModel},
		{Model: config.ClaudeModel},
		{Model: config.GPT56SolModel}, // duplicate
		{Model: ""},                   // skipped
	}
	got := sessionview.JoinLabels(sessionview.EngineLabels(sessions), " + ")
	want := config.SolLabel + " + " + config.ClaudeLabel
	if got != want {
		t.Errorf("EngineLabels joined = %q, want %q", got, want)
	}
}

func TestGroupElapsedUsesWallClockSpanAcrossSequentialMembers(t *testing.T) {
	start := time.Now().Add(-10 * time.Minute).Truncate(time.Second)
	reviewerEnd := start.Add(4 * time.Minute)
	judgeStart := reviewerEnd
	judgeEnd := judgeStart.Add(3 * time.Minute)

	got := sessionview.Elapsed([]*session.Session{
		{Status: "completed", StartTime: start, EndTime: &reviewerEnd, Duration: "4m0s"},
		{Status: "failed", StartTime: judgeStart, EndTime: &judgeEnd, Duration: "3m0s"},
	})
	if got != "7m0s" {
		t.Fatalf("sessionview.Elapsed() = %q, want wall-clock span %q", got, "7m0s")
	}
}

func TestPublicSessionsUsesOnlyPublicNames(t *testing.T) {
	original := &session.Session{CLI: "codex", Model: config.GPT56SolModel, ErrorMsg: "Codex CLI failed for " + config.GPT56SolModel}
	got := publicSessions([]*session.Session{original})
	if len(got) != 1 || got[0].CLI != config.SolLabel || got[0].Model != config.SolLabel {
		t.Fatalf("public session = %+v, want sol labels", got)
	}
	if original.CLI != "codex" || original.Model != config.GPT56SolModel {
		t.Fatal("publicSessions mutated persisted session metadata")
	}
	if strings.Contains(got[0].ErrorMsg, "Codex") || strings.Contains(got[0].ErrorMsg, config.GPT56SolModel) {
		t.Fatalf("public session error was not normalized: %q", got[0].ErrorMsg)
	}
}

// serveLog writes one session (metadata + log file) into a temp HOME and
// returns what GET /api/sessions/<id>/log actually serves.
func serveLog(t *testing.T, id string, s *session.Session, raw []byte) *httptest.ResponseRecorder {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	dir := config.SessionDirPath()
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	s.ID = id
	s.LogFile = filepath.Join(dir, id+".log")
	if s.Status == "" {
		s.Status = "completed"
	}
	if s.StartTime.IsZero() {
		s.StartTime = time.Now()
	}
	if err := s.Save(); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(s.LogFile, raw, 0600); err != nil {
		t.Fatal(err)
	}

	req := httptest.NewRequest(http.MethodGet, "/api/sessions/"+id+"/log", nil)
	rr := httptest.NewRecorder()
	New("test").ServeHTTP(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("log status = %d, body = %s", rr.Code, rr.Body.String())
	}
	return rr
}

func TestLogEndpointNormalizesRuntimeBanner(t *testing.T) {
	raw := []byte("OpenAI Codex v1\n--------\nmodel: " + config.GPT56SolModel + "\n--------\n")
	rr := serveLog(t, "00000000-0000-4000-8000-0000000000a1",
		&session.Session{CLI: "codex", Model: config.GPT56SolModel}, raw)
	got := rr.Body.String()
	if strings.Contains(got, config.GPT56SolModel) || strings.Contains(got, "Codex") || !strings.Contains(got, "Sol runtime") {
		t.Fatalf("public log was not normalized: %q", got)
	}
}

// The web page renders the served bytes as text: an ESC leaks through as an
// invisible byte plus a literal "[31m", a "\r" concatenates every progress
// frame into one absurd line, and C0 controls become replacement boxes. Tabs
// must survive — CSS tab-size renders them, and expanding here would fight it.
func TestLogEndpointSanitizesControlBytesAndKeepsTabs(t *testing.T) {
	raw := []byte("\x1b[31mRED\x1b[0m\nprog 10%\rprog 99%\nbell\x07here\n\tindented\n")
	rr := serveLog(t, "00000000-0000-4000-8000-0000000000a2",
		&session.Session{CLI: "codex", Model: config.GPT56SolModel}, raw)
	got := rr.Body.String()

	for _, banned := range []string{"\x1b", "\r", "\x07", "[31m", "[0m"} {
		if strings.Contains(got, banned) {
			t.Fatalf("served log still contains %q: %q", banned, got)
		}
	}
	for _, r := range got {
		if r != '\n' && r != '\t' && (r < 0x20 || r == 0x7f) {
			t.Fatalf("served log contains control char %#U: %q", r, got)
		}
	}
	if !strings.Contains(got, "RED") || !strings.Contains(got, "bellhere") {
		t.Fatalf("sanitizer ate visible text: %q", got)
	}
	if strings.Contains(got, "prog 10%") || !strings.Contains(got, "prog 99%") {
		t.Fatalf("progress frames not collapsed to the last: %q", got)
	}
	if !strings.Contains(got, "\tindented") {
		t.Fatalf("tab was not preserved for the web path: %q", got)
	}
}

// replaceConcreteModelIDs is a plain string replacement, so an ANSI escape
// interleaved in a model id used to smuggle the internal id straight to the
// browser. Sanitizing before redacting closes that.
func TestLogEndpointRedactsModelIDSplitByANSI(t *testing.T) {
	split := config.GPT56SolModel[:3] + "\x1b[0m" + config.GPT56SolModel[3:]
	raw := []byte("model: " + split + "\n")
	rr := serveLog(t, "00000000-0000-4000-8000-0000000000a3",
		&session.Session{CLI: "codex", Model: config.GPT56SolModel}, raw)
	got := rr.Body.String()
	if strings.Contains(got, config.GPT56SolModel) {
		t.Fatalf("internal model id leaked past ANSI-split redaction: %q", got)
	}
	// The escape is invisible in a browser, so an unsanitized response reads as
	// the intact internal id even though a byte-wise Contains misses it.
	// Dropping the ESC bytes reconstructs exactly what the user would see.
	visible := strings.ReplaceAll(got, "\x1b", "")
	if strings.Contains(visible, config.GPT56SolModel[:3]+"[0m"+config.GPT56SolModel[3:]) {
		t.Fatalf("internal model id readable once the invisible ESC is dropped: %q", got)
	}
	if !strings.Contains(got, config.SolLabel) {
		t.Fatalf("public label missing: %q", got)
	}
}

// A blind seek to size-maxBytes lands mid-rune and mid-escape. Aligning the
// tail to the next newline fixes both, since escapes never span a line here.
func TestReadLogTailAlignsToLineBoundary(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "session.log")

	// One long line of multi-byte runes plus an ANSI escape, repeated until the
	// file is well over the cap, so the cut point lands inside a line.
	line := "\x1b[31m" + strings.Repeat("あ", 64) + "\x1b[0m tail marker\n"
	var b strings.Builder
	for b.Len() < int(maxLogTailBytes)*2 {
		b.WriteString(line)
	}
	if err := os.WriteFile(path, []byte(b.String()), 0600); err != nil {
		t.Fatal(err)
	}

	tail, truncated, err := logfmt.ReadTail(path, maxLogTailBytes)
	if err != nil {
		t.Fatal(err)
	}
	if !truncated {
		t.Fatal("oversized log must report truncated")
	}
	if len(tail) > int(maxLogTailBytes) {
		t.Fatalf("tail = %d bytes, want <= %d", len(tail), maxLogTailBytes)
	}
	if !utf8.Valid(tail) {
		t.Fatal("tail is not valid UTF-8")
	}
	// Line-aligned: the tail must begin a whole line, i.e. with the escape that
	// opens every line — never mid-rune or mid-escape.
	if !strings.HasPrefix(string(tail), "\x1b[31m") {
		t.Fatalf("tail does not start on a line boundary: %q", string(tail)[:32])
	}
	// A beheaded escape would leave a literal "[0m"/"[31m" with no ESC before
	// it. Every "[" in the aligned tail must be preceded by an ESC.
	for i, r := range string(tail) {
		if r == '[' && (i == 0 || tail[i-1] != 0x1b) {
			t.Fatalf("dangling escape remnant at %d: %q", i, string(tail)[max(0, i-8):i+8])
		}
	}
	if strings.Count(string(tail), "\x1b[31m") != strings.Count(string(tail), "\x1b[0m") {
		t.Fatal("unbalanced escapes: tail was cut inside a line")
	}
}

// A single enormous line has no newline to align to; serving the unaligned tail
// beats serving nothing at all.
func TestReadLogTailFallsBackWhenNoNewline(t *testing.T) {
	path := filepath.Join(t.TempDir(), "huge.log")
	raw := strings.Repeat("x", int(maxLogTailBytes)*2) + "END"
	if err := os.WriteFile(path, []byte(raw), 0600); err != nil {
		t.Fatal(err)
	}
	tail, truncated, err := logfmt.ReadTail(path, maxLogTailBytes)
	if err != nil {
		t.Fatal(err)
	}
	if !truncated || len(tail) == 0 {
		t.Fatalf("no-newline fallback: truncated=%v bytes=%d", truncated, len(tail))
	}
	if !strings.HasSuffix(string(tail), "END") {
		t.Fatal("no-newline fallback did not serve the end of the log")
	}
}

// A tail whose only newline is the final byte would align to an empty
// remainder; the fallback must keep serving the unaligned bytes.
func TestReadLogTailFallsBackWhenAlignmentEmptiesTail(t *testing.T) {
	path := filepath.Join(t.TempDir(), "trailing-newline.log")
	raw := strings.Repeat("y", int(maxLogTailBytes)*2) + "\n"
	if err := os.WriteFile(path, []byte(raw), 0600); err != nil {
		t.Fatal(err)
	}
	tail, truncated, err := logfmt.ReadTail(path, maxLogTailBytes)
	if err != nil {
		t.Fatal(err)
	}
	if !truncated || len(tail) == 0 {
		t.Fatalf("empty-after-alignment fallback: truncated=%v bytes=%d", truncated, len(tail))
	}
}

func TestSessionsAPIIsBoundedAndExcludesPrivateFields(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	dir := config.SessionDirPath()
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}

	ids := []string{
		"00000000-0000-4000-8000-000000000001",
		"00000000-0000-4000-8000-000000000002",
		"00000000-0000-4000-8000-000000000003",
	}
	for i, id := range ids {
		s := &session.Session{
			ID:            id,
			CLI:           "opencode",
			Mode:          "raw",
			Model:         "test-model",
			Effort:        "high",
			Prompt:        "private-prompt-" + id,
			PromptPreview: "preview-" + id,
			Status:        "completed",
			StartTime:     time.Now().Add(time.Duration(i) * time.Minute),
			WorkDir:       "/private/workdir",
			LogFile:       "/private/internal.log",
			PID:           4242,
		}
		if err := s.Save(); err != nil {
			t.Fatal(err)
		}
	}

	req := httptest.NewRequest(http.MethodGet, "/api/sessions?limit=1&offset=1", nil)
	rr := httptest.NewRecorder()
	New("test-version").ServeHTTP(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", rr.Code, rr.Body.String())
	}

	var got apiResponse
	if err := json.Unmarshal(rr.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if len(got.Groups) != 1 || got.Pagination.Shown != 1 || got.Pagination.Offset != 1 || got.Pagination.TotalGroups != 3 {
		t.Fatalf("pagination response = %+v, groups = %d", got.Pagination, len(got.Groups))
	}
	if !got.Pagination.HasMore || !got.Pagination.HasPrevious {
		t.Fatalf("pagination directions = %+v", got.Pagination)
	}
	if got.Groups[0].Sessions[0].ID != ids[1] {
		t.Fatalf("middle page id = %q, want %q", got.Groups[0].Sessions[0].ID, ids[1])
	}

	body := rr.Body.String()
	for _, private := range []string{"private-prompt-", "/private/internal.log", `"pid"`, `"log_file"`} {
		if strings.Contains(body, private) {
			t.Errorf("list API leaked private field %q", private)
		}
	}
}

func TestRunAPIResolvesGroupOutsideListPagination(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	if err := os.MkdirAll(config.SessionDirPath(), 0700); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	for index, id := range []string{
		"00000000-0000-4000-8000-000000000011",
		"00000000-0000-4000-8000-000000000012",
	} {
		s := &session.Session{
			ID:            id,
			GroupID:       "older-group",
			CLI:           "codex",
			Mode:          "megareview",
			Model:         config.GPT56SolModel,
			Effort:        "high",
			PromptPreview: "group prompt",
			Status:        []string{"completed", "failed"}[index],
			StartTime:     now.Add(time.Duration(index) * time.Second),
			WorkDir:       "/work",
		}
		if err := s.Save(); err != nil {
			t.Fatal(err)
		}
	}

	mux := New("test")
	req := httptest.NewRequest(http.MethodGet, "/api/run?id=older-group", nil)
	rr := httptest.NewRecorder()
	mux.ServeHTTP(rr, req)
	if rr.Code != http.StatusOK {
		t.Fatalf("run status = %d, body = %s", rr.Code, rr.Body.String())
	}
	var group sessionGroup
	if err := json.Unmarshal(rr.Body.Bytes(), &group); err != nil {
		t.Fatal(err)
	}
	if group.ID != "older-group" || len(group.Sessions) != 2 || group.Status != "failed" {
		t.Fatalf("resolved group = %+v", group)
	}

	for _, tc := range []struct {
		method string
		target string
		want   int
	}{
		{http.MethodGet, "/api/run?id=missing", http.StatusNotFound},
		{http.MethodGet, "/api/run", http.StatusBadRequest},
		{http.MethodPost, "/api/run?id=older-group", http.StatusMethodNotAllowed},
	} {
		req := httptest.NewRequest(tc.method, tc.target, nil)
		rr := httptest.NewRecorder()
		mux.ServeHTTP(rr, req)
		if rr.Code != tc.want {
			t.Errorf("%s %s status = %d, want %d", tc.method, tc.target, rr.Code, tc.want)
		}
	}
}

func TestPromptResponseAndLogTailAreBounded(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "session.json")
	largePrompt := strings.Repeat("prompt", (maxPromptSourceFileBytes/6)+1)
	stored := session.Session{Prompt: largePrompt}
	data, err := json.MarshalIndent(stored, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}

	detail, err := loadPromptResponse(path, &session.Session{PromptPreview: "saved preview"})
	if err != nil {
		t.Fatal(err)
	}
	if !detail.Truncated || !detail.PreviewOnly || detail.Prompt != "saved preview" {
		t.Fatalf("large prompt response = %+v", detail)
	}

	logPath := filepath.Join(dir, "session.log")
	logData := []byte("prefix€" + strings.Repeat("x", maxLogTailBytes) + "tail")
	if err := os.WriteFile(logPath, logData, 0600); err != nil {
		t.Fatal(err)
	}
	tail, truncated, err := logfmt.ReadTail(logPath, maxLogTailBytes)
	if err != nil {
		t.Fatal(err)
	}
	if !truncated || len(tail) > maxLogTailBytes || !utf8.Valid(tail) || !strings.HasSuffix(string(tail), "tail") {
		t.Fatalf("log tail: truncated=%v bytes=%d utf8=%v suffix=%q", truncated, len(tail), utf8.Valid(tail), tail[max(0, len(tail)-8):])
	}
}

func TestListPageValidationAndCap(t *testing.T) {
	for _, tc := range []struct {
		target     string
		wantLimit  int
		wantOffset int
		wantErr    bool
	}{
		{target: "/api/sessions", wantLimit: defaultListLimit},
		{target: "/api/sessions?limit=99999&offset=12", wantLimit: maxListLimit, wantOffset: 12},
		{target: "/api/sessions?limit=0", wantErr: true},
		{target: "/api/sessions?offset=-1", wantErr: true},
	} {
		req := httptest.NewRequest(http.MethodGet, tc.target, nil)
		limit, offset, err := listPage(req)
		if (err != nil) != tc.wantErr {
			t.Fatalf("listPage(%q) error = %v, wantErr %v", tc.target, err, tc.wantErr)
		}
		if err == nil && (limit != tc.wantLimit || offset != tc.wantOffset) {
			t.Fatalf("listPage(%q) = (%d, %d), want (%d, %d)", tc.target, limit, offset, tc.wantLimit, tc.wantOffset)
		}
	}
}

// An antislop run must reach the API as its own kind. It previously reused
// the plan mode, so the dashboard labelled it a plan review.
func TestGroupKindReportsAntislop(t *testing.T) {
	sessions := []*session.Session{
		{ID: "a", GroupID: "g", Mode: session.ModeAntislop, Status: "completed", CLI: "codex", Model: config.GPT56SolModel},
		{ID: "b", GroupID: "g", Mode: session.ModeAntislop, Status: "completed", CLI: "claude", Model: config.ClaudeModel},
	}
	groups := groupSessions(sessions)
	if len(groups) != 1 {
		t.Fatalf("got %d groups, want 1", len(groups))
	}
	if groups[0].Kind != "antislop" {
		t.Errorf("group kind = %q, want antislop", groups[0].Kind)
	}
}
