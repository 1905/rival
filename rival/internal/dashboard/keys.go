package dashboard

import (
	"charm.land/bubbles/v2/help"
	"charm.land/bubbles/v2/key"
)

// mode is which part of the UI owns the keyboard. Routing by mode, not by
// key-switch order, is what stops a list key ("j", "q") leaking into a text
// input or a viewport.
type mode int

const (
	modeList    mode = iota // list has focus
	modeFilter              // the list filter input has focus
	modeDetail              // the detail screen has focus
	modeSearch              // the detail search input has focus
	modeConfirm             // a y/n confirm bar is open
)

// keyMap holds every binding in the TUI. Filter and Search share "/", and
// NextPage (list), NextMatch (detail) and No (confirm) share "n": only one
// mode is active at a time, so they never collide.
type keyMap struct {
	Up, Down, Top, Bottom, NextPage, PrevPage, Open, Back, Filter, NextTab, PrevTab, Help, Quit,
	Follow, Search, NextMatch, PrevMatch, NextMember, PrevMember,
	TabOutput, TabPrompt, TabInfo, OpenLog, Stop, Yes, No key.Binding
}

func defaultKeys() keyMap {
	return keyMap{
		Up:         key.NewBinding(key.WithKeys("k", "up"), key.WithHelp("↑/k", "up")),
		Down:       key.NewBinding(key.WithKeys("j", "down"), key.WithHelp("↓/j", "down")),
		Top:        key.NewBinding(key.WithKeys("g", "home"), key.WithHelp("g", "top")),
		Bottom:     key.NewBinding(key.WithKeys("G", "end"), key.WithHelp("G", "bottom")),
		NextPage:   key.NewBinding(key.WithKeys("n", "pgdown"), key.WithHelp("n/pgdn", "next page")),
		PrevPage:   key.NewBinding(key.WithKeys("p", "pgup"), key.WithHelp("p/pgup", "prev page")),
		Open:       key.NewBinding(key.WithKeys("enter"), key.WithHelp("enter", "open")),
		Back:       key.NewBinding(key.WithKeys("esc"), key.WithHelp("esc", "back")),
		Filter:     key.NewBinding(key.WithKeys("/"), key.WithHelp("/", "filter")),
		NextTab:    key.NewBinding(key.WithKeys("tab"), key.WithHelp("tab", "status")),
		PrevTab:    key.NewBinding(key.WithKeys("shift+tab"), key.WithHelp("shift+tab", "prev status")),
		Help:       key.NewBinding(key.WithKeys("?"), key.WithHelp("?", "more")),
		Quit:       key.NewBinding(key.WithKeys("q", "ctrl+c"), key.WithHelp("q", "quit")),
		Follow:     key.NewBinding(key.WithKeys("f"), key.WithHelp("f", "follow")),
		Search:     key.NewBinding(key.WithKeys("/"), key.WithHelp("/", "search")),
		NextMatch:  key.NewBinding(key.WithKeys("n"), key.WithHelp("n", "next match")),
		PrevMatch:  key.NewBinding(key.WithKeys("N"), key.WithHelp("N", "prev match")),
		NextMember: key.NewBinding(key.WithKeys("]"), key.WithHelp("]", "next member")),
		PrevMember: key.NewBinding(key.WithKeys("["), key.WithHelp("[", "prev member")),
		TabOutput:  key.NewBinding(key.WithKeys("1"), key.WithHelp("1", "output")),
		TabPrompt:  key.NewBinding(key.WithKeys("2"), key.WithHelp("2", "prompt")),
		TabInfo:    key.NewBinding(key.WithKeys("3"), key.WithHelp("3", "info")),
		OpenLog:    key.NewBinding(key.WithKeys("o"), key.WithHelp("o", "open log")),
		Stop:       key.NewBinding(key.WithKeys("x"), key.WithHelp("x", "stop")),
		Yes:        key.NewBinding(key.WithKeys("y"), key.WithHelp("y", "yes")),
		No:         key.NewBinding(key.WithKeys("n", "esc"), key.WithHelp("n/esc", "no")),
	}
}

// forceQuit quits from any mode, including the text inputs where "q" is a
// character rather than a command.
var forceQuit = key.NewBinding(key.WithKeys("ctrl+c"))

// modeHelp is a help.KeyMap for one mode.
type modeHelp struct {
	short []key.Binding
	full  [][]key.Binding
}

func (h modeHelp) ShortHelp() []key.Binding  { return h.short }
func (h modeHelp) FullHelp() [][]key.Binding { return h.full }

// simpleHelp is a one-row help: the same bindings short and expanded.
func simpleHelp(bs ...key.Binding) modeHelp {
	return modeHelp{short: bs, full: [][]key.Binding{bs}}
}

// relabel returns a copy of b with a mode-specific description. Bindings are
// values, so this never changes the shared keyMap.
func relabel(b key.Binding, desc string) key.Binding {
	b.SetHelp(b.Help().Key, desc)
	return b
}

// help returns the bindings to advertise in mode m.
func (k keyMap) help(m mode) help.KeyMap {
	switch m {
	case modeFilter:
		return simpleHelp(relabel(k.Open, "keep filter"), relabel(k.Back, "clear"))
	case modeDetail:
		tabs := key.NewBinding(key.WithKeys("1", "2", "3"), key.WithHelp("1-3", "tab"))
		members := key.NewBinding(key.WithKeys("[", "]"), key.WithHelp("[/]", "member"))
		return modeHelp{
			short: []key.Binding{tabs, members, k.Follow, k.Search, k.OpenLog, k.Stop, k.Back, k.Help},
			full: [][]key.Binding{
				{k.Up, k.Down, k.Top, k.Bottom, k.Follow},
				{k.TabOutput, k.TabPrompt, k.TabInfo, k.NextMember, k.PrevMember},
				{k.Search, k.NextMatch, k.PrevMatch},
				{k.OpenLog, k.Stop, k.Back, tabs, k.Help, k.Quit},
			},
		}
	case modeSearch:
		return simpleHelp(relabel(k.Open, "search"), relabel(k.Back, "clear"))
	case modeConfirm:
		return simpleHelp(k.Yes, k.No)
	default:
		pages := key.NewBinding(key.WithKeys("n", "p"), key.WithHelp("n/p", "page"))
		return modeHelp{
			short: []key.Binding{k.Up, k.Down, pages, k.Open, k.Filter, k.NextTab, k.Help, k.Quit},
			full: [][]key.Binding{
				{k.Up, k.Down, k.Top, k.Bottom},
				{k.NextPage, k.PrevPage},
				{k.Open, k.Filter, k.Back},
				{k.NextTab, k.PrevTab},
				{k.Help, k.Quit},
			},
		}
	}
}
