package dashboard

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"syscall"
	"time"

	"charm.land/bubbles/v2/help"
	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/1905/rival/internal/procinfo"
	"github.com/1905/rival/internal/session"
	"github.com/1905/rival/internal/sessionview"
	"github.com/charmbracelet/x/ansi"
)

// displayItem wraps one or more sessions for display in the TUI.
type displayItem struct {
	Sessions []*session.Session
	// hay caches the lowercase filter text. Items are rebuilt on every
	// SessionEvent, so the cache never outlives the data it was built from,
	// and each filter keystroke skips re-deriving 3000 haystacks.
	hay string
}

// Primary returns the first session (used for shared metadata).
func (d *displayItem) Primary() *session.Session {
	if len(d.Sessions) == 0 {
		return nil
	}
	return d.Sessions[0]
}

// IsGroup returns true for a logical grouped run, including a degraded run
// where only one requested model passed preflight.
func (d *displayItem) IsGroup() bool {
	return len(d.Sessions) > 1 || (len(d.Sessions) == 1 && d.Sessions[0].GroupID != "")
}

// groupSessions merges sessions sharing a GroupID into display items. The
// bucketing itself lives in internal/sessionview.
func groupSessions(sessions []*session.Session) []displayItem {
	buckets := sessionview.Group(sessions)
	items := make([]displayItem, 0, len(buckets))
	for _, b := range buckets {
		items = append(items, toDisplayItem(b))
	}
	return items
}

// toDisplayItem adapts a shared bucket to the TUI's row type.
func toDisplayItem(b sessionview.Bucket) displayItem {
	return displayItem{Sessions: b.Sessions}
}

// Model is the bubbletea model for the TUI dashboard.
type Model struct {
	lay layout
	// helpH is the help bar's height in rows. It depends on the mode, "?" and
	// the width, so measureHelp recomputes it when one of those changes rather
	// than rendering the full help on every geometry call.
	helpH  int
	events chan SessionEvent
	// progress carries the initial scan's LoadProgress; WatchSessions closes
	// it when that scan is done.
	progress chan LoadProgress
	// loaded turns true on the first SessionEvent. Until then the body is the
	// loader and the counts are "…", never a misleading 0.
	loaded   bool
	load     LoadProgress
	ctx      context.Context
	cancel   context.CancelFunc
	errText  string
	quitting bool

	keys    keyMap
	help    help.Model
	mode    mode
	list    listPane
	preview previewPane

	// spin animates running rows. It ticks only while something runs, so an
	// idle dashboard costs no CPU; spinning records whether a tick is in
	// flight, so a new event never starts a second, faster chain.
	spin     spinner.Model
	spinning bool
	// ticking is the same guard for the 1s refresh tick: every SessionEvent
	// used to start another self-renewing chain, multiplying log reads.
	ticking bool

	// kill signals a process. Tests replace it to record signals instead of
	// sending them.
	kill func(pid int, sig syscall.Signal) error
	// alive reports whether pid is still the process that started at start.
	// It authorizes stop signals, so the default is procinfo.SameProcess,
	// which never passes an unrecorded start. Tests replace it with fakes;
	// mayStop also refuses PIDStart 0 whatever alive says.
	alive func(pid int, start int64) bool

	// detail is the full-screen view of the selected run. modeDetail,
	// modeSearch and modeConfirm all draw it.
	detail detailPane
}

// Version is set from cmd package before launching the TUI.
var Version = "dev"

// New creates a new dashboard model.
func New() Model {
	events := make(chan SessionEvent, 10)
	ctx, cancel := context.WithCancel(context.Background())
	h := help.New()
	h.Styles = help.Styles{
		Ellipsis:       dimStyle,
		ShortKey:       textStyle,
		ShortDesc:      dimStyle,
		ShortSeparator: dimStyle,
		FullKey:        textStyle,
		FullDesc:       dimStyle,
		FullSeparator:  dimStyle,
	}
	h.ShortSeparator = " · "
	m := Model{
		events:   events,
		progress: make(chan LoadProgress, 1),
		// Init starts the spinner chain for the loader.
		spinning: true,
		ctx:      ctx,
		cancel:   cancel,
		keys:     defaultKeys(),
		help:     h,
		list:     newListPane(),
		// No style: the frame is embedded in rows that pick their own colour
		// (and the selection bar must not get a nested reset).
		spin:   spinner.New(spinner.WithSpinner(spinner.MiniDot)),
		kill:   syscall.Kill,
		alive:  procinfo.SameProcess,
		detail: newDetailPane(),
	}
	m.measureHelp()
	return m
}

// itemKey identifies a display item across refreshes. A group is keyed by its
// GroupID, a solo session by its own ID; both are stable for the run's lifetime.
func itemKey(item *displayItem) string {
	s := item.Primary()
	if s == nil {
		return ""
	}
	if s.GroupID != "" {
		return "group:" + s.GroupID
	}
	return "solo:" + s.ID
}

// applySessions rebuilds the list from a watcher snapshot. The list keeps the
// cursor on the same run; when that run vanished while its detail view was
// open, the user drops back to the list rather than seeing another run's log
// under the old heading (and "x" pointing at the wrong PID).
func (m *Model) applySessions(sessions []*session.Session) {
	anchor := m.list.selectedKey()
	m.list.setItems(groupSessions(sessions), time.Now())
	if m.inDetail() {
		if m.list.selectedKey() != anchor {
			m.closeDetail()
		} else {
			m.syncDetail(false)
		}
	}
}

// inDetail reports whether the detail screen is showing: browsing it, typing a
// search, or answering the stop confirm.
func (m Model) inDetail() bool {
	return m.mode == modeDetail || m.mode == modeSearch || m.mode == modeConfirm
}

func (m *Model) closeDetail() {
	m.mode = modeList
	m.detail.close()
}

// helpView renders the help bar for the current mode, no line wider than the
// terminal. lipgloss.JoinVertical pads every row to the widest line, so one
// over-wide help line would drag the whole frame over-width.
func (m Model) helpView() string {
	h := m.help
	h.SetWidth(m.lay.Width)
	lines := strings.Split(h.View(m.keys.help(m.mode)), "\n")
	for i, l := range lines {
		lines[i] = ansi.Truncate(l, m.lay.Width, "")
	}
	return strings.Join(lines, "\n")
}

// measureHelp stores how many rows the help bar takes; "?" expands it. Call
// it whenever the mode, "?" or the width changes.
func (m *Model) measureHelp() {
	m.helpH = strings.Count(m.helpView(), "\n") + 1
}

// listBodyHeight is the list pane height: the layout body minus any rows the
// expanded help borrows.
func (m Model) listBodyHeight() int {
	return max(1, m.lay.BodyH-(m.helpH-1))
}

// listInnerHeight and listInnerWidth are the list pane's size inside its
// border. The border exists only in the split view; alone, the list takes the
// whole body.
func (m Model) listInnerHeight() int {
	if m.lay.ShowPreview {
		return max(1, m.listBodyHeight()-2)
	}
	return m.listBodyHeight()
}

func (m Model) listInnerWidth() int {
	if m.lay.ShowPreview {
		return max(1, m.lay.ListW-2)
	}
	return m.lay.Width
}

// listRows is how many data rows fit between the list's column titles and
// its page footer.
func (m Model) listRows() int {
	return max(1, m.listInnerHeight()-2)
}

// syncPreview re-renders the preview for the selected run. Its log is re-read
// only when the file changed. The detail screen hides the preview, so nothing
// is read there.
func (m *Model) syncPreview() {
	if !m.lay.ShowPreview || m.lay.TooSmall || m.inDetail() {
		return
	}
	m.preview.refresh(m.list.selected(), m.previewInnerWidth(), m.listInnerHeight())
}

// previewInnerWidth is the preview text width: the box minus its border and
// a 1-col pad each side, so the text never touches the border.
func (m Model) previewInnerWidth() int {
	return max(1, m.lay.PreviewW-4)
}

// contentHeight is the detail body height: everything between the header and
// the help bar. Both viewContent and syncDetail MUST use it: if the two ever
// compute a different height the viewport renders a frame the view then
// clips, which is what leaves stale rows on screen.
func (m Model) contentHeight() int {
	return max(0, m.lay.Height-m.lay.HeaderH-m.helpH)
}

// syncDetail resizes the detail viewport and reloads its content for the
// current selection. reset puts the scroll back to the tab's start (the tail
// for Output); otherwise follow decides.
func (m *Model) syncDetail(reset bool) {
	if !m.inDetail() {
		return
	}
	m.resizeDetail()
	m.detail.reload(m.list.selected(), m.lay.Width, reset)
}

// resizeDetail fits the detail viewport to the current geometry without
// re-reading anything.
func (m *Model) resizeDetail() {
	if m.inDetail() {
		m.detail.resize(m.lay.Width, m.contentHeight())
	}
}

// Init starts the file watcher and waits for events, the loader's progress
// and its spinner.
func (m Model) Init() tea.Cmd {
	watch := func() tea.Msg {
		if err := WatchSessions(m.ctx, m.events, m.progress); err != nil {
			return errMsg{err}
		}
		return <-m.events
	}
	return tea.Batch(watch, waitForProgress(m.progress), m.spin.Tick)
}

// waitForProgress delivers the next LoadProgress. It returns nil once
// WatchSessions closes the channel after the initial scan, ending the chain.
func waitForProgress(ch chan LoadProgress) tea.Cmd {
	return func() tea.Msg {
		p, ok := <-ch
		if !ok {
			return nil
		}
		return p
	}
}

type errMsg struct{ error }

// tickMsg fires periodically to refresh live timers and log tails.
type tickMsg time.Time

func tickCmd() tea.Cmd {
	return tea.Tick(time.Second, func(t time.Time) tea.Msg {
		return tickMsg(t)
	})
}

func waitForEvent(events chan SessionEvent) tea.Cmd {
	return func() tea.Msg {
		return <-events
	}
}

// ensureLiveTimers starts whichever of the 1s refresh tick and the spinner is
// not already running, while anything is live. Each chain renews itself and
// ends on its own once nothing runs, so starting a second one would double
// its rate.
func (m *Model) ensureLiveTimers() tea.Cmd {
	if !m.list.anyLive {
		return nil
	}
	var cmds []tea.Cmd
	if !m.ticking {
		m.ticking = true
		cmds = append(cmds, tickCmd())
	}
	if !m.spinning {
		m.spinning = true
		cmds = append(cmds, m.spin.Tick)
	}
	return tea.Batch(cmds...)
}

func (m Model) quit() (tea.Model, tea.Cmd) {
	m.quitting = true
	if m.cancel != nil {
		m.cancel()
	}
	return m, tea.Quit
}

// Update handles messages.
func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	prevMode, prevHelp := m.mode, m.help.ShowAll
	next, cmd := m.update(msg)
	nm, ok := next.(Model)
	if !ok {
		return next, cmd
	}
	// The help bar's height depends on the mode and on "?". When either
	// changes, the detail viewport must be resized, or it keeps a height the
	// view then clips and the log's last lines vanish.
	if nm.mode != prevMode || nm.help.ShowAll != prevHelp {
		(&nm).measureHelp()
		(&nm).resizeDetail()
	}
	// A spinner frame changes nothing the list or the preview show.
	if _, spin := msg.(spinner.TickMsg); !spin {
		(&nm).reconcile()
	}
	return nm, cmd
}

// reconcile runs once after every update: it scrolls the list so the cursor
// stays in view and re-renders the preview for whatever is selected now.
func (m *Model) reconcile() {
	m.list.clampOffset(m.listRows())
	m.syncPreview()
}

// updateFilterInput feeds msg to the filter input and refilters when the
// text changed. Keys and pastes both land here.
func (m *Model) updateFilterInput(msg tea.Msg) tea.Cmd {
	before := m.list.filter.Value()
	var cmd tea.Cmd
	m.list.filter, cmd = m.list.filter.Update(msg)
	if m.list.filter.Value() != before {
		m.list.refilter(time.Now())
	}
	return cmd
}

func (m Model) update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyPressMsg:
		if key.Matches(msg, forceQuit) {
			return m.quit()
		}
		switch m.mode {
		case modeFilter:
			return m.updateFilterKey(msg)
		case modeDetail:
			return m.updateDetailKey(msg)
		case modeSearch:
			return m.updateSearchKey(msg)
		case modeConfirm:
			return m.updateConfirmKey(msg)
		default:
			return m.updateListKey(msg)
		}

	case tea.WindowSizeMsg:
		m.lay = computeLayout(msg.Width, msg.Height)
		(&m).measureHelp()
		(&m).syncDetail(false)

	case SessionEvent:
		m.loaded = true
		(&m).applySessions(msg.Sessions)
		return m, tea.Batch(waitForEvent(m.events), (&m).ensureLiveTimers())

	case tickMsg:
		// Re-read the open run's log while it can still grow. Keep ticking
		// while anything runs.
		if sel := m.list.selected(); m.inDetail() && sel != nil && itemLive(sel) {
			(&m).syncDetail(false)
		}
		if m.list.anyLive {
			return m, tickCmd()
		}
		m.ticking = false
		return m, nil

	case LoadProgress:
		if m.loaded {
			return m, nil
		}
		m.load = msg
		return m, waitForProgress(m.progress)

	case spinner.TickMsg:
		// Dropping the tick ends the chain; the next SessionEvent with a
		// running row restarts it. The loader keeps it alive until then.
		if m.loaded && !m.list.anyLive {
			m.spinning = false
			return m, nil
		}
		var cmd tea.Cmd
		m.spin, cmd = m.spin.Update(msg)
		return m, cmd

	case errMsg:
		m.errText = msg.Error()
		// No snapshot is coming; let the loader's spinner chain end.
		m.loaded = true
		return m, nil

	default:
		// Cursor blink and other input-internal messages.
		var cmd tea.Cmd
		switch m.mode {
		case modeFilter:
			// A paste arrives here, not as a key, so it must refilter too.
			cmd = (&m).updateFilterInput(msg)
		case modeSearch:
			m.detail.search, cmd = m.detail.search.Update(msg)
		}
		return m, cmd
	}

	return m, nil
}

func (m Model) updateListKey(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	k := m.keys
	switch {
	case key.Matches(msg, k.Quit):
		return m.quit()
	case key.Matches(msg, k.Up):
		m.list.move(-1)
	case key.Matches(msg, k.Down):
		m.list.move(1)
	case key.Matches(msg, k.Top):
		m.list.top()
	case key.Matches(msg, k.Bottom):
		m.list.bottom()
	case key.Matches(msg, k.NextPage):
		m.list.turnPage(1)
	case key.Matches(msg, k.PrevPage):
		m.list.turnPage(-1)
	case key.Matches(msg, k.NextTab):
		m.list.cycleTab(1, time.Now())
	case key.Matches(msg, k.PrevTab):
		m.list.cycleTab(-1, time.Now())
	case key.Matches(msg, k.Help):
		m.help.ShowAll = !m.help.ShowAll
	case key.Matches(msg, k.Filter):
		m.mode = modeFilter
		cmd := m.list.filter.Focus()
		return m, cmd
	case key.Matches(msg, k.Back):
		// esc outside the input clears a kept filter.
		if m.list.filter.Value() != "" {
			m.list.filter.Reset()
			m.list.refilter(time.Now())
		}
	case key.Matches(msg, k.Open):
		if sel := m.list.selected(); sel != nil {
			m.mode = modeDetail
			m.detail.open(sel)
			// Open at the tail: live output and final verdicts both live at
			// the end of the log.
			(&m).syncDetail(true)
		}
		return m, nil
	}
	return m, nil
}

// Arrow keys still move the cursor while the filter has focus; j/k are
// letters there.
var (
	arrowUp   = key.NewBinding(key.WithKeys("up"))
	arrowDown = key.NewBinding(key.WithKeys("down"))
)

func (m Model) updateFilterKey(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	switch {
	case key.Matches(msg, m.keys.Open):
		m.list.filter.Blur()
		m.mode = modeList
	case key.Matches(msg, m.keys.Back):
		m.list.filter.Reset()
		m.list.filter.Blur()
		m.list.refilter(time.Now())
		m.mode = modeList
	case key.Matches(msg, arrowUp):
		m.list.move(-1)
	case key.Matches(msg, arrowDown):
		m.list.move(1)
	default:
		cmd = (&m).updateFilterInput(msg)
	}
	return m, cmd
}

// updateDetailKey drives the detail screen while it has focus.
func (m Model) updateDetailKey(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	k := m.keys
	d := &m.detail
	item := m.list.selected()
	d.notice = ""
	switch {
	case key.Matches(msg, k.Quit):
		return m.quit()

	case key.Matches(msg, k.Back), msg.String() == "backspace":
		// esc peels one layer: a search first, then the screen.
		if d.query != "" {
			d.clearSearch()
			d.applyContent()
			return m, nil
		}
		(&m).closeDetail()
		return m, nil

	case key.Matches(msg, k.Help):
		m.help.ShowAll = !m.help.ShowAll

	case key.Matches(msg, k.TabOutput):
		d.tab = tabOutput
		(&m).syncDetail(true)
	case key.Matches(msg, k.TabPrompt):
		d.tab = tabPrompt
		(&m).syncDetail(true)
	case key.Matches(msg, k.TabInfo):
		d.tab = tabInfo
		(&m).syncDetail(true)
	case key.Matches(msg, k.NextTab):
		d.tab = (d.tab + 1) % detailTabCount
		(&m).syncDetail(true)
	case key.Matches(msg, k.PrevTab):
		d.tab = (d.tab + detailTabCount - 1) % detailTabCount
		(&m).syncDetail(true)

	case key.Matches(msg, k.NextMember):
		d.cycleMember(item, 1)
		(&m).syncDetail(true)
	case key.Matches(msg, k.PrevMember):
		d.cycleMember(item, -1)
		(&m).syncDetail(true)

	case key.Matches(msg, k.Follow), key.Matches(msg, k.Bottom):
		d.follow = true
		d.vp.GotoBottom()
	case key.Matches(msg, k.Top):
		d.vp.GotoTop()
		d.follow = d.vp.AtBottom()

	case key.Matches(msg, k.Search):
		m.mode = modeSearch
		d.search.SetValue(d.query)
		d.search.CursorEnd()
		return m, d.search.Focus()
	case key.Matches(msg, k.NextMatch):
		d.stepMatch(1)
	case key.Matches(msg, k.PrevMatch):
		d.stepMatch(-1)

	case key.Matches(msg, k.OpenLog):
		if item != nil {
			if item.IsGroup() {
				openGroupLogs(item.Sessions)
			} else if s := item.Primary(); s != nil && s.LogFile != "" {
				openLog(s)
			}
		}

	case key.Matches(msg, k.Stop):
		// Never signal on one key: x only opens the confirm bar.
		if targets := liveTargets(item, m.alive); len(targets) > 0 {
			d.confirm = &killConfirm{targets: targets}
			m.mode = modeConfirm
		} else if hasUnverified(item) {
			d.notice = unverifiedNotice
		} else {
			d.notice = "nothing running"
		}

	default:
		// Scroll keys (j/k/up/down/pgup/pgdn/space/u/d) belong to the
		// viewport. Scrolling away from the tail pauses follow; scrolling
		// back down to it resumes.
		var cmd tea.Cmd
		d.vp, cmd = d.vp.Update(msg)
		if d.tab == tabOutput {
			d.follow = d.vp.AtBottom()
		}
		return m, cmd
	}
	return m, nil
}

// updateSearchKey drives the search input. Every key but enter, esc and
// ctrl+c is text, so "q" and "n" type themselves.
func (m Model) updateSearchKey(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	d := &m.detail
	switch {
	case key.Matches(msg, m.keys.Open):
		d.search.Blur()
		m.mode = modeDetail
		d.runSearch(d.search.Value())
	case key.Matches(msg, m.keys.Back):
		d.clearSearch()
		d.applyContent()
		m.mode = modeDetail
	default:
		var cmd tea.Cmd
		d.search, cmd = d.search.Update(msg)
		return m, cmd
	}
	return m, nil
}

// updateConfirmKey answers the stop confirm. Only y stops; any other key
// cancels, so a stray keypress can never kill a run.
func (m Model) updateConfirmKey(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	confirm := m.detail.confirm
	m.detail.confirm = nil
	m.mode = modeDetail
	if confirm == nil || !key.Matches(msg, m.keys.Yes) {
		return m, nil
	}
	// Re-check against the current snapshot: a target that finished while
	// the bar was open must not get a signal (its PID may be reused). A
	// target still "running" whose process died meanwhile goes through, so
	// stopSessions can fail it as dead.
	want := make(map[string]bool, len(confirm.targets))
	for _, s := range confirm.targets {
		want[s.ID] = true
	}
	var targets []*session.Session
	if item := m.list.selected(); item != nil {
		for _, s := range item.Sessions {
			if want[s.ID] && isLive(s.Status) && s.PID > 0 {
				targets = append(targets, s)
			}
		}
	}
	if len(targets) == 0 {
		m.detail.notice = "nothing running"
		return m, nil
	}
	m.stopSessions(targets)
	// The targets were failed in place; the header must count them now.
	m.list.countStatuses()
	(&m).syncDetail(false)
	return m, nil
}

// stopSessions sends SIGTERM to each target and marks it failed so the TUI
// updates at once. Only a target mayStop confirms gets the signal; any other
// (dead, PID reused, or no recorded PIDStart) is failed as already dead.
//
// The owning rival process also finalizes a signalled session on SIGTERM.
// Both writers go through Save's unique temp file + rename, so they can no
// longer interleave into partial JSON; the last rename wins with a whole file.
func (m Model) stopSessions(targets []*session.Session) {
	for _, s := range targets {
		// liveTargets already checked, but the process can die between the
		// confirm and y, and its PID can then go to an unrelated process.
		if s.PIDStart == 0 {
			// No recorded start time: a dead run and a live one look the
			// same, so neither signal nor rewrite it. The owner or the
			// reaper finalizes it.
			continue
		}
		if !mayStop(s, m.alive) || m.kill(s.PID, syscall.SIGTERM) != nil {
			// The recorded process is gone (the PID may even belong to
			// another process now): never signal, mark it dead.
			failSessionForKill(s, 1, "killed (process already dead)")
			continue
		}
		// Signal sent — mark failed so the TUI updates instantly. The
		// subprocess executor will overwrite with its own status.
		failSessionForKill(s, 137, "killed by user")
	}
}

// openLog and openGroupLogs copy the raw log, with no public model renaming,
// to a temp file and open it, so the model id in the file matches the one on
// screen.
func openLog(s *session.Session) {
	viewPath, err := createLogView(s)
	if err != nil {
		return
	}
	openLogPath(viewPath)
}

func openGroupLogs(sessions []*session.Session) {
	viewPath, err := createGroupLogView(sessions)
	if err != nil {
		return
	}
	openLogPath(viewPath)
}

func openLogPath(viewPath string) {
	if err := exec.Command("open", viewPath).Start(); err != nil {
		_ = os.Remove(viewPath)
		return
	}
	time.AfterFunc(10*time.Minute, func() { _ = os.Remove(viewPath) })
}

func createLogView(s *session.Session) (string, error) {
	data, err := os.ReadFile(s.LogFile)
	if err != nil {
		return "", err
	}
	return createTextView(string(data))
}

func createGroupLogView(sessions []*session.Session) (string, error) {
	var content strings.Builder
	for _, s := range sessions {
		label := groupLogLabel(s)
		if s.Status == "failed" && s.ErrorMsg != "" {
			label += " (FAILED)"
		}
		fmt.Fprintf(&content, "=== %s ===\n", label)
		if s.ErrorMsg != "" {
			fmt.Fprintf(&content, "Error: %s\n", s.ErrorMsg)
		}
		data, err := os.ReadFile(s.LogFile)
		if err != nil {
			if s.ErrorMsg == "" {
				fmt.Fprintf(&content, "(log unavailable: %s)\n", err)
			}
		} else {
			content.Write(data)
			if len(data) > 0 && data[len(data)-1] != '\n' {
				content.WriteByte('\n')
			}
		}
		content.WriteByte('\n')
	}
	return createTextView(content.String())
}

// groupLogLabel heads one member in the combined group log: its raw model id,
// its role and its effort.
func groupLogLabel(s *session.Session) string {
	role := "REVIEW"
	if s.Mode == "consilium" {
		role = "JUDGE"
	}
	label := modelName(s) + " " + role
	if s.Effort != "" {
		label += " · EFFORT " + s.Effort
	}
	return label
}

func createTextView(content string) (string, error) {
	file, err := os.CreateTemp("", "rival-log-*.txt")
	if err != nil {
		return "", err
	}
	viewPath := file.Name()
	if _, err := file.WriteString(content); err != nil {
		_ = file.Close()
		_ = os.Remove(viewPath)
		return "", err
	}
	if err := file.Close(); err != nil {
		_ = os.Remove(viewPath)
		return "", err
	}
	return viewPath, nil
}

// headerStats are the session counts for the header, counted per event.
func (m Model) headerStats() headerStats {
	st := m.list.stats
	st.Version = Version
	st.Loading = !m.loaded
	return st
}

// spinFrame is the current spinner frame, or "" when nothing runs.
func (m Model) spinFrame() string {
	if !m.loaded {
		return m.spin.View()
	}
	if !m.list.anyLive {
		return ""
	}
	return m.spin.View()
}

// View renders the UI. AltScreen is set on the view (bubbletea v2 dropped the
// tea.WithAltScreen program option).
func (m Model) View() tea.View {
	return altScreenView(m.viewContent())
}

// altScreenView wraps rendered content in an alt-screen tea.View.
func altScreenView(content string) tea.View {
	v := tea.NewView(content)
	v.AltScreen = true
	return v
}

// tooSmallNotice replaces the frame when the terminal cannot hold it.
var tooSmallNotice = fmt.Sprintf("terminal too small (need %d×%d)", minWidth, minHeight)

// viewContent renders the frame body.
func (m Model) viewContent() string {
	if m.quitting {
		return ""
	}

	if m.errText != "" {
		return "Error: " + m.errText
	}

	if m.lay.Width == 0 || m.lay.Height == 0 {
		return "Initializing..."
	}

	if m.lay.TooSmall {
		return ansi.Truncate(tooSmallNotice, m.lay.Width, "")
	}

	spin := m.spinFrame()
	header := renderHeader(m.lay.Width, m.lay.Compact, m.headerStats(), spin)
	helpBar := m.helpView()

	if m.inDetail() {
		// Update owns the log content — the view only draws what the viewport
		// already holds, so no file is read here.
		body := m.detail.view(m.list.selected(), m.lay.Width, m.contentHeight(), spin)
		return strings.Join([]string{header, body, helpBar}, "\n")
	}
	tabs := m.list.tabBar(m.lay.Width, !m.loaded)
	body := m.loaderView(spin)
	if m.loaded {
		body = m.bodyView(spin)
	}
	return strings.Join([]string{header, tabs, body, helpBar}, "\n")
}

// bodyView is the list alone, or, from previewMinWidth up, the list and the
// preview in rounded boxes with a 1-col gap. Both boxes are exactly
// listBodyHeight rows. The list always has focus here, so its border is
// accent and the preview's is dim.
func (m Model) bodyView(spin string) string {
	list := m.list.view(m.listInnerWidth(), m.listInnerHeight(), spin)
	if !m.lay.ShowPreview {
		return list
	}
	left := borderStyle.BorderForeground(colAccent).Render(list)
	right := borderStyle.Padding(0, 1).Render(m.preview.view(m.previewInnerWidth(), m.listInnerHeight()))
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
}

// loaderBarMax caps the loader bar's width so it reads as a bar, not a rule.
const loaderBarMax = 48

// loaderView fills the list body until the first snapshot: the spinner and
// "reading sessions done/total" over a bar in the logo gradient, centred in
// exactly width × listBodyHeight cells.
func (m Model) loaderView(spin string) string {
	w, h := m.lay.Width, m.listBodyHeight()
	label := "reading sessions"
	pct := 0.0
	if m.load.Total > 0 {
		label += fmt.Sprintf(" %d/%d", m.load.Done, m.load.Total)
		pct = float64(m.load.Done) / float64(m.load.Total)
	}
	barW := min(loaderBarMax, max(1, w-4))
	title := runningStyle.Render(spin) + " " + textStyle.Render(label)
	lines := []string{ansi.Truncate(title, barW, "")}
	if h >= 3 {
		lines = append(lines, "", gradientBar(barW, pct))
	}
	block := lipgloss.NewStyle().Width(barW).Align(lipgloss.Center).Render(strings.Join(lines, "\n"))
	return lipgloss.Place(w, h, lipgloss.Center, lipgloss.Center, block)
}
