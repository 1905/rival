package dashboard

import (
	"fmt"
	"image/color"
	"strings"
	"sync"

	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"
)

// Phosphor palette. Colours are truecolor; lipgloss downsamples them on
// 256/16-colour terminals, so no fallback table is kept here.
var (
	colFg      color.Color = lipgloss.Color("#B8FFB8") // body text
	colDim     color.Color = lipgloss.Color("#3E6B4A") // secondary text, borders, help
	colAccent  color.Color = lipgloss.Color("#39FF14") // selection bar, active tab, focus border
	colRunning color.Color = lipgloss.Color("#FFB000") // amber: spinner, running status
	colQueued  color.Color = lipgloss.Color("#6B8F7A") // grey-green: waiting in line
	colOK      color.Color = lipgloss.Color("#39FF14") // completed
	colFail    color.Color = lipgloss.Color("#FF3B3B") // failed
)

// logoStops are the gradient stops for the ASCII logo: violet → cyan → green.
var logoStops = []color.Color{
	lipgloss.Color("#7C3AED"),
	lipgloss.Color("#22D3EE"),
	lipgloss.Color("#39FF14"),
}

var (
	textStyle = lipgloss.NewStyle().Foreground(colFg)
	dimStyle  = lipgloss.NewStyle().Foreground(colDim)

	// selectedStyle is the cursor bar. Black on accent reads on every
	// background, and bold keeps it legible once lipgloss downsamples.
	selectedStyle = lipgloss.NewStyle().
			Bold(true).
			Foreground(lipgloss.Color("#000000")).
			Background(colAccent)

	borderStyle = lipgloss.NewStyle().
			Border(lipgloss.RoundedBorder()).
			BorderForeground(colDim)

	// Column titles and section rows in the list.
	headerStyle  = lipgloss.NewStyle().Foreground(colDim).Bold(true)
	sectionStyle = lipgloss.NewStyle().Foreground(colAccent).Bold(true)

	activeTabStyle   = lipgloss.NewStyle().Foreground(colAccent).Bold(true).Underline(true)
	inactiveTabStyle = dimStyle

	runningStyle   = lipgloss.NewStyle().Foreground(colRunning).Bold(true)
	completedStyle = lipgloss.NewStyle().Foreground(colOK)
	failedStyle    = lipgloss.NewStyle().Foreground(colFail)
	queuedStyle    = lipgloss.NewStyle().Foreground(colQueued)

	// labelStyle marks notes inside log text; valueStyle is an emphasised
	// value (model id, session id).
	labelStyle = dimStyle
	valueStyle = lipgloss.NewStyle().Bold(true).Foreground(colFg)
)

func statusStyle(status string) lipgloss.Style {
	switch status {
	case "running":
		return runningStyle
	case "completed":
		return completedStyle
	case "failed":
		return failedStyle
	case "queued":
		return queuedStyle
	default:
		return textStyle
	}
}

// bannerLines is the ASCII logo for the TUI header.
var bannerLines = []string{
	`         _             __`,
	`   _____(_)   ______ _/ /`,
	"  / ___/ / | / / __ `/ /",
	` / /  / /| |/ / /_/ / /`,
	`/_/  /_/ |___/\__,_/_/`,
}

// bannerWidth is computed from the actual banner lines.
var bannerWidth = func() int {
	w := 0
	for _, l := range bannerLines {
		if n := len([]rune(l)); n > w {
			w = n
		}
	}
	return w
}()

var (
	logoOnce     sync.Once
	logoRendered string
)

// renderLogo returns the banner with a 45° gradient, one colour per cell.
// Every line is padded to bannerWidth so the stats column beside it lines up.
// It is built once: the logo never changes and per-cell styling is the most
// expensive thing in the header.
func renderLogo() string {
	logoOnce.Do(func() {
		h := len(bannerLines)
		grad := lipgloss.Blend2D(bannerWidth, h, 45, logoStops...)
		var b strings.Builder
		for y, line := range bannerLines {
			runes := []rune(line)
			for x := 0; x < bannerWidth; x++ {
				if x >= len(runes) || runes[x] == ' ' {
					b.WriteByte(' ')
					continue
				}
				b.WriteString(lipgloss.NewStyle().Bold(true).Foreground(grad[y*bannerWidth+x]).Render(string(runes[x])))
			}
			if y < h-1 {
				b.WriteByte('\n')
			}
		}
		logoRendered = b.String()
	})
	return logoRendered
}

// headerStats are the session counts shown beside the logo.
type headerStats struct {
	Running, Queued, Completed, Failed, Total int
	Version                                   string
	// Loading shows "…" for every count until the first snapshot.
	Loading bool
}

// compactHeaderBelowHeight is the terminal height under which the 5-row logo
// collapses to a one-line header, so short terminals keep their rows for runs.
const compactHeaderBelowHeight = 30

// renderHeader draws the header block. Wide form: the gradient logo on the
// left and the stats right-aligned beside it. Compact form: one line with
// "rival" in the gradient plus the same stats. No line exceeds width.
func renderHeader(width int, compact bool, st headerStats, spin string) string {
	if width <= 0 {
		return ""
	}
	if spin == "" {
		spin = "●"
	}
	n := func(v int) string {
		if st.Loading {
			return "…"
		}
		return fmt.Sprint(v)
	}
	running := runningStyle.Render(fmt.Sprintf("%s %s running", spin, n(st.Running)))
	queued := queuedStyle.Render(fmt.Sprintf("◌ %s queued", n(st.Queued)))
	done := completedStyle.Render("✓ " + n(st.Completed))
	failed := failedStyle.Render("✗ " + n(st.Failed))
	version := dimStyle.Render(st.Version)
	total := dimStyle.Render(n(st.Total) + " sessions")

	if compact {
		line := rivalWord() + "  " + strings.Join([]string{running, queued, done, failed, total, version}, "  ")
		return ansi.Truncate(line, width, "")
	}

	stats := []string{
		"",
		running + "  " + queued,
		done + "  " + failed,
		total + "  " + version,
		"",
	}
	statsW := 0
	for _, s := range stats {
		statsW = max(statsW, lipgloss.Width(s))
	}
	logo := strings.Split(renderLogo(), "\n")
	out := make([]string, len(logo))
	for i, l := range logo {
		s := stats[i]
		// Right-align the stats block as a whole, so its lines share a left edge.
		gap := width - bannerWidth - statsW
		if gap < 2 {
			gap = 2
		}
		line := l
		if s != "" {
			line += strings.Repeat(" ", gap) + s
		}
		out[i] = ansi.Truncate(line, width, "")
	}
	return strings.Join(out, "\n")
}

var (
	rivalOnce     sync.Once
	rivalRendered string
)

// rivalWord is gradientWord("rival"), built once like the logo: the header
// and the breadcrumb draw it on every frame.
func rivalWord() string {
	rivalOnce.Do(func() { rivalRendered = gradientWord("rival") })
	return rivalRendered
}

// gradientWord colours each rune of word along the logo gradient.
func gradientWord(word string) string {
	runes := []rune(word)
	cols := lipgloss.Blend1D(len(runes), logoStops...)
	var b strings.Builder
	for i, r := range runes {
		b.WriteString(lipgloss.NewStyle().Bold(true).Foreground(cols[i]).Render(string(r)))
	}
	return b.String()
}

// gradientBar is a width-cell progress bar: the filled part in the logo
// gradient (violet → cyan → green across the whole bar), the rest dim.
// bubbles/progress can blend too, but it pulls in harmonica for animation this
// static bar does not need.
func gradientBar(width int, pct float64) string {
	if width <= 0 {
		return ""
	}
	filled := int(pct*float64(width) + 0.5)
	filled = min(max(filled, 0), width)
	cols := lipgloss.Blend1D(width, logoStops...)
	var b strings.Builder
	for i := 0; i < filled; i++ {
		b.WriteString(lipgloss.NewStyle().Foreground(cols[i]).Render("█"))
	}
	b.WriteString(dimStyle.Render(strings.Repeat("░", width-filled)))
	return b.String()
}
