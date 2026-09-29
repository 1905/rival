package dashboard

import (
	"strings"
	"testing"
	"time"

	"charm.land/lipgloss/v2"
	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
)

func TestGroupEffortShowsMixedDefaults(t *testing.T) {
	item := &displayItem{Sessions: []*session.Session{
		{Effort: "ultra"},
		{Effort: "low"},
	}}
	if got := groupEffort(item); got != "mixed" {
		t.Fatalf("groupEffort() = %q, want mixed", got)
	}
	item.Sessions[1].Effort = "ultra"
	if got := groupEffort(item); got != "ultra" {
		t.Fatalf("groupEffort() = %q, want ultra", got)
	}
}

func TestPairedPlanGroupShowsRawModelsAndPlanKind(t *testing.T) {
	created := time.Now()
	later := created.Add(time.Millisecond)
	items := groupSessions([]*session.Session{
		// LoadAll returns newest first; grouping must restore requested order.
		{ID: "b", GroupID: "paired", CLI: "claude", Model: config.ClaudeModel, Mode: "plan", QueuedAt: &later},
		{ID: "a", GroupID: "paired", CLI: "codex", Model: config.GPT56SolModel, Mode: "plan", QueuedAt: &created},
	})
	if len(items) != 1 || !items[0].IsGroup() {
		t.Fatalf("paired plan TUI items = %+v, want one group", items)
	}
	item := &items[0]
	if got := kindLabel(item); got != "plan" {
		t.Fatalf("paired plan kind = %q, want plan", got)
	}
	if got, want := groupModelName(item), config.GPT56SolModel+" +1"; got != want {
		t.Fatalf("paired plan model = %q, want %q (first requested model, raw id)", got, want)
	}
}

func TestSingletonClaudePlanRemainsLogicalPlanGroup(t *testing.T) {
	items := groupSessions([]*session.Session{
		{ID: "claude", GroupID: "degraded-plan", CLI: "claude", Model: config.ClaudeModel, Mode: "plan", Status: "running"},
	})
	if len(items) != 1 || !items[0].IsGroup() {
		t.Fatalf("singleton plan item = %+v, want logical group", items)
	}
	if got := kindLabel(&items[0]); got != "plan" {
		t.Fatalf("singleton plan kind = %q", got)
	}
	if got := groupModelName(&items[0]); got != config.ClaudeModel {
		t.Fatalf("singleton plan model = %q, want %q", got, config.ClaudeModel)
	}
}

// Same fixtures the removed cliLabel test covered: every CLI now shows its raw
// model id, and a live Claude plan run is kind "plan".
func TestRowLabelsForEveryCLI(t *testing.T) {
	for _, s := range []*session.Session{
		{CLI: "codex", Model: config.GPT56SolModel, Mode: "review"},
		{CLI: "opencode", Model: config.KimiModel, Mode: "review"},
		{CLI: "claude", Model: config.ClaudeModel, Mode: "review"},
		{CLI: "grok", Model: config.GrokModel, Mode: "review"},
	} {
		item := &displayItem{Sessions: []*session.Session{s}}
		if got := groupModelName(item); got != s.Model {
			t.Errorf("%s model cell = %q, want raw id %q", s.CLI, got, s.Model)
		}
		if got := kindLabel(item); got != "review" {
			t.Errorf("%s kind = %q, want review", s.CLI, got)
		}
	}
	plan := &displayItem{Sessions: []*session.Session{{CLI: "claude", Model: config.ClaudeModel, Mode: "plan"}}}
	if got := kindLabel(plan); got != "plan" {
		t.Errorf("live Claude plan kind = %q, want plan", got)
	}
}

func TestModelNameShowsTheRawID(t *testing.T) {
	tests := []struct {
		s    session.Session
		want string
	}{
		{session.Session{CLI: "codex", Model: "gpt-6-astra"}, "gpt-6-astra"},
		{session.Session{CLI: "claude", Model: "claude-fable-5"}, "claude-fable-5"},
		{session.Session{CLI: "codex", Model: "gpt-5.5"}, "gpt-5.5"},
		{session.Session{CLI: "codex", Model: ""}, "codex"},
	}
	for _, tc := range tests {
		if got := modelName(&tc.s); got != tc.want {
			t.Errorf("modelName(%+v) = %q, want %q", tc.s, got, tc.want)
		}
	}
}

func TestGroupModelName(t *testing.T) {
	tests := []struct {
		name string
		item displayItem
		want string
	}{
		{"three distinct models", displayItem{Sessions: []*session.Session{
			{GroupID: "g", Model: "gpt-5.5", Mode: "megareview"},
			{GroupID: "g", Model: "gemini-3.1", Mode: "megareview"},
			{GroupID: "g", Model: "kimi-k3", Mode: "consilium"},
		}}, "gpt-5.5 +2"},
		{"judge reuses a reviewer model", displayItem{Sessions: []*session.Session{
			{GroupID: "g", Model: "gpt-6-astra", Mode: "megareview"},
			{GroupID: "g", Model: "gpt-6-astra", Mode: "consilium"},
		}}, "gpt-6-astra"},
		{"solo", displayItem{Sessions: []*session.Session{{Model: "claude-opus-5-5"}}}, "claude-opus-5-5"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := groupModelName(&tc.item); got != tc.want {
				t.Fatalf("groupModelName = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestKindLabel(t *testing.T) {
	solo := func(cli, mode string) displayItem {
		return displayItem{Sessions: []*session.Session{{CLI: cli, Mode: mode}}}
	}
	group := func(modes ...string) displayItem {
		var ss []*session.Session
		for _, m := range modes {
			ss = append(ss, &session.Session{GroupID: "g", Mode: m})
		}
		return displayItem{Sessions: ss}
	}
	tests := []struct {
		name string
		item displayItem
		want string
	}{
		{"review", solo("codex", "review"), "review"},
		{"plan", solo("codex", "plan"), "plan"},
		{"security", solo("opencode", session.ModeSecurity), "sec"},
		{"antislop", solo("codex", session.ModeAntislop), "slop"},
		{"raw", solo("opencode", "raw"), "raw"},
		{"native", solo("claude", "native"), "review"},
		{"empty mode", solo("codex", ""), "review"},
		{"docker claude", solo("claude", "docker"), "review/dk"},
		{"docker fable (read-compat)", solo("fable", "docker"), "review/dk"},
		{"megareview group", group("megareview", "megareview", "consilium"), "mega"},
		{"plan group", group("plan", "plan"), "plan"},
		{"antislop group", group(session.ModeAntislop, session.ModeAntislop), "slop"},
		{"security group", group(session.ModeSecurity), "sec"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := kindLabel(&tc.item); got != tc.want {
				t.Fatalf("kindLabel = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestProjectName(t *testing.T) {
	for in, want := range map[string]string{
		"/a/b/orbit-web":  "orbit-web",
		"/a/b/orbit-web/": "orbit-web",
		"":                  "-",
	} {
		if got := projectName(in); got != want {
			t.Errorf("projectName(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestSectionFor(t *testing.T) {
	now := time.Date(2026, 9, 26, 11, 40, 0, 0, time.Local)
	tests := []struct {
		t    time.Time
		want string
	}{
		{now.Add(-time.Hour), "TODAY"},
		{time.Date(2026, 9, 26, 0, 0, 1, 0, time.Local), "TODAY"},
		{time.Date(2026, 9, 25, 23, 59, 0, 0, time.Local), "YESTERDAY"},
		{now.AddDate(0, 0, -3), "THIS WEEK"},
		{now.AddDate(0, 0, -30), "OLDER"},
		{time.Time{}, "OLDER"},
	}
	for _, tc := range tests {
		if got := sectionFor(tc.t, now); got != tc.want {
			t.Errorf("sectionFor(%v) = %q, want %q", tc.t, got, tc.want)
		}
	}
}

func filterFixture(now time.Time) []displayItem {
	return []displayItem{
		{Sessions: []*session.Session{{ID: "aaaaaaaa-1", CLI: "codex", Model: "gpt-6-astra", Mode: "review", Effort: "xhigh", Status: "running", StartTime: now.Add(-time.Minute), WorkDir: "/src/orbit-web", PromptPreview: "review the fingerprint re-key"}}},
		{Sessions: []*session.Session{{ID: "bbbbbbbb-2", CLI: "claude", Model: "claude-opus-5-5", Mode: "plan", Effort: "medium", Status: "completed", StartTime: now.Add(-2 * time.Hour), WorkDir: "/src/ledger", ReviewScope: "plans/2026-09-26-service-identity"}}},
		{Sessions: []*session.Session{{ID: "cccccccc-3", CLI: "codex", Model: "gpt-5.5", Mode: "review", Effort: "high", Status: "failed", StartTime: now.AddDate(0, 0, -1), WorkDir: "/src/orbit-web"}}},
		{Sessions: []*session.Session{{ID: "dddddddd-4", CLI: "codex", Model: "gpt-5.5", Mode: "review", Effort: "high", Status: "completed", StartTime: now.AddDate(0, 0, -40), WorkDir: "/src/mathquest"}}},
	}
}

func TestMatchesFilter(t *testing.T) {
	now := time.Now()
	items := filterFixture(now)
	tests := []struct {
		terms []string
		want  []bool
	}{
		{nil, []bool{true, true, true, true}},
		{[]string{"orbit"}, []bool{true, false, true, false}},
		{[]string{"orbit", "failed"}, []bool{false, false, true, false}},
		{[]string{"opus"}, []bool{false, true, false, false}},
		{[]string{"fingerprint"}, []bool{true, false, false, false}},
		{[]string{"service-identity"}, []bool{false, true, false, false}},
		{[]string{"cccccccc"}, []bool{false, false, true, false}},
		{[]string{"plan"}, []bool{false, true, false, false}},
		{[]string{"xhigh"}, []bool{true, false, false, false}},
		{[]string{"nothing-matches"}, []bool{false, false, false, false}},
	}
	for _, tc := range tests {
		for i := range items {
			if got := matchesFilter(&items[i], tc.terms); got != tc.want[i] {
				t.Errorf("matchesFilter(item %d, %q) = %v, want %v", i, tc.terms, got, tc.want[i])
			}
		}
	}
}

func rowSummary(rows []row) []string {
	var out []string
	for _, r := range rows {
		if r.Section != "" {
			out = append(out, "#"+r.Section)
			continue
		}
		out = append(out, r.Item.Primary().ID[:1])
	}
	return out
}

// noonToday pins "now" to midday so fixtures offset by hours never cross
// midnight and flip section.
func noonToday() time.Time {
	y, m, d := time.Now().Date()
	return time.Date(y, m, d, 12, 0, 0, 0, time.Local)
}

// rows is rowsAndCounts without the counts, with a raw filter string.
func rows(items []displayItem, tab statusTab, filter string, now time.Time) []row {
	r, _ := rowsAndCounts(items, tab, filterTerms(filter), now)
	return r
}

func TestBuildRows(t *testing.T) {
	now := noonToday()
	items := filterFixture(now)
	yesterday := "#YESTERDAY"
	if sectionFor(now.AddDate(0, 0, -1), now) != "YESTERDAY" {
		t.Fatal("fixture premise broken")
	}
	tests := []struct {
		name   string
		tab    statusTab
		filter string
		want   []string
	}{
		{"all, no filter", tabAll, "", []string{"#TODAY", "a", "b", yesterday, "c", "#OLDER", "d"}},
		{"filter case-insensitive", tabAll, "ORBIT", []string{"#TODAY", "a", yesterday, "c"}},
		{"running tab", tabRunning, "", []string{"#TODAY", "a"}},
		{"failed tab", tabFailed, "", []string{yesterday, "c"}},
		{"done tab", tabDone, "", []string{"#TODAY", "b", "#OLDER", "d"}},
		{"tab composes with filter", tabDone, "gpt-5.5", []string{"#OLDER", "d"}},
		{"no match", tabAll, "zzz", nil},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := rowSummary(rows(items, tc.tab, tc.filter, now))
			if strings.Join(got, ",") != strings.Join(tc.want, ",") {
				t.Fatalf("rows = %v, want %v", got, tc.want)
			}
		})
	}
}

// Out-of-order input must still yield one header per section, in order.
func TestBuildRowsGroupsOutOfOrderItems(t *testing.T) {
	now := noonToday()
	items := filterFixture(now)
	shuffled := []displayItem{items[3], items[0], items[2], items[1]}
	got := rowSummary(rows(shuffled, tabAll, "", now))
	want := []string{"#TODAY", "a", "b", "#YESTERDAY", "c", "#OLDER", "d"}
	if strings.Join(got, ",") != strings.Join(want, ",") {
		t.Fatalf("rows = %v, want %v", got, want)
	}
}

func BenchmarkBuildRows3000(b *testing.B) {
	now := time.Now()
	base := filterFixture(now)
	items := make([]displayItem, 0, 3000)
	for i := 0; i < 3000; i++ {
		src := base[i%len(base)].Primary()
		s := *src
		s.StartTime = now.Add(-time.Duration(i) * time.Hour)
		s.PromptPreview = strings.Repeat("review the whole repository for bugs ", 3)
		items = append(items, displayItem{Sessions: []*session.Session{&s}})
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = rowsAndCounts(items, tabAll, filterTerms("orbit high"), now)
	}
}

func TestLayoutColumnsDropsEffortThenProject(t *testing.T) {
	if c := layoutColumns(120); c.Effort == 0 || c.Project == 0 {
		t.Fatalf("layoutColumns(120) = %+v, want effort and project", c)
	}
	if c := layoutColumns(68); c.Effort == 0 || c.Project == 0 {
		t.Fatalf("layoutColumns(68) = %+v, want every fixed column to fit", c)
	}
	if c := layoutColumns(67); c.Effort != 0 || c.Project == 0 {
		t.Fatalf("layoutColumns(67) = %+v, want effort dropped, project kept", c)
	}
	if c := layoutColumns(59); c.Effort != 0 || c.Project != 0 {
		t.Fatalf("layoutColumns(59) = %+v, want effort and project dropped", c)
	}
	for _, w := range []int{60, 70, 89, 90, 120, 200} {
		c := layoutColumns(w)
		sum := 1
		for _, cw := range []int{c.Status, c.Kind, c.Model, c.Effort, c.Time, c.Project} {
			if cw > 0 {
				sum += cw + 1
			}
		}
		// PROJECT absorbs the spare width, so the row is exactly the pane
		// width plus its trailing gap; there is no prompt column any more.
		if c.Project > 0 && sum-1 != w {
			t.Fatalf("layoutColumns(%d) fills %d cells: %+v", w, sum-1, c)
		}
	}
}

func TestFitCell(t *testing.T) {
	tests := []struct {
		in   string
		w    int
		want string
	}{
		{"abc", 5, "abc  "},
		{"abcdef", 4, "abc…"},
		{"日本語", 4, "日… "},
		{"✓", 3, "✓  "},
		{"x", 0, ""},
	}
	for _, tc := range tests {
		got := fitCell(tc.in, tc.w)
		if got != tc.want {
			t.Errorf("fitCell(%q, %d) = %q, want %q", tc.in, tc.w, got, tc.want)
		}
		if w := lipgloss.Width(got); w != tc.w {
			t.Errorf("fitCell(%q, %d) width = %d", tc.in, tc.w, w)
		}
	}
}

func TestStatusGlyph(t *testing.T) {
	for status, want := range map[string]string{
		"running": "⠋", "queued": "◌", "completed": "✓", "failed": "✗", "killed": "·", "": "·",
	} {
		if got := statusGlyph(status, "⠋"); got != want {
			t.Errorf("statusGlyph(%q) = %q, want %q", status, got, want)
		}
	}
}

// Regression for misaligned rows: every rendered line must be exactly the pane
// width, whatever multi-byte or wide text sits in its cells.
func TestRenderedRowsFillExactlyThePaneWidth(t *testing.T) {
	now := noonToday()
	q := now.Add(-time.Minute)
	items := []displayItem{
		{Sessions: []*session.Session{{ID: "a1", CLI: "claude", Model: "claude-opus-4-6[1m]", Mode: "docker", Effort: "max", Status: "running", StartTime: now.Add(-time.Minute), WorkDir: "/src/日本語のプロジェクト", PromptPreview: strings.Repeat("長い prompt ", 30)}}},
		{Sessions: []*session.Session{{ID: "b2", CLI: "codex", Model: "gpt-6-astra", Mode: "review", Effort: "xhigh", Status: "completed", Duration: "6m24s", StartTime: now.Add(-time.Hour), WorkDir: "/src/disk-watcher", PromptPreview: strings.Repeat("p", 300)}}},
		{Sessions: []*session.Session{{ID: "c3", CLI: "codex", Model: "gpt-5.5", Mode: "review", Status: "queued", QueuePosition: 3, QueuedAt: &q, WorkDir: "/x"}}},
		{Sessions: []*session.Session{
			{ID: "d4", GroupID: "g", CLI: "codex", Model: "gpt-5.5", Mode: "megareview", Effort: "high", Status: "failed", StartTime: now.AddDate(0, 0, -1), WorkDir: "/src/mathquest"},
			{ID: "d5", GroupID: "g", CLI: "opencode", Model: "kimi-k3", Mode: "megareview", Effort: "high", Status: "completed", StartTime: now.AddDate(0, 0, -1)},
		}},
	}
	for _, width := range []int{60, 90, 120, 200} {
		var l listPane
		l.filter = newFilterInput()
		l.setItems(items, now)
		out := l.view(width, 20, "⠋")
		lines := strings.Split(out, "\n")
		if len(lines) != 20 {
			t.Fatalf("width %d: view has %d lines, want 20", width, len(lines))
		}
		for i, line := range lines {
			if w := lipgloss.Width(line); w != width {
				t.Errorf("width %d: line %d is %d cells: %q", width, i, w, line)
			}
		}
	}
}

// The prompt is noise in the list (every review starts with the same
// boilerplate), so no row or header may show it.
func TestListShowsNoPrompt(t *testing.T) {
	sessions := previewFixture(t)
	for _, s := range sessions {
		s.PromptPreview = "BOILERPLATE-PROMPT"
	}
	frame := frameText(newListModel(t, sessions, 200, 40))
	if strings.Contains(frame, "BOILERPLATE-PROMPT") || strings.Contains(frame, "PROMPT") {
		t.Fatalf("list still shows the prompt:\n%s", frame)
	}
}
