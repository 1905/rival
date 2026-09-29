package dashboard

import (
	"reflect"
	"testing"

	"charm.land/bubbles/v2/key"
)

func TestDefaultKeysAreAllBoundAndDocumented(t *testing.T) {
	k := defaultKeys()
	v := reflect.ValueOf(k)
	for i := 0; i < v.NumField(); i++ {
		name := v.Type().Field(i).Name
		b := v.Field(i).Interface().(key.Binding)
		if len(b.Keys()) == 0 {
			t.Errorf("%s has no keys", name)
		}
		if b.Help().Key == "" || b.Help().Desc == "" {
			t.Errorf("%s has no help text: %+v", name, b.Help())
		}
	}
}

// bindingSet flattens bindings into their help-desc identity. Bindings hold
// slices, so they are compared by keys + help rather than by ==.
func hasBinding(list []key.Binding, want key.Binding) bool {
	for _, b := range list {
		if reflect.DeepEqual(b.Keys(), want.Keys()) && b.Help().Key == want.Help().Key {
			return true
		}
	}
	return false
}

func flatten(groups [][]key.Binding) []key.Binding {
	var out []key.Binding
	for _, g := range groups {
		out = append(out, g...)
	}
	return out
}

func TestListShortHelp(t *testing.T) {
	k := defaultKeys()
	short := k.help(modeList).ShortHelp()
	for name, b := range map[string]key.Binding{
		"Up": k.Up, "Down": k.Down, "Open": k.Open, "Filter": k.Filter,
		"NextTab": k.NextTab, "Help": k.Help, "Quit": k.Quit,
	} {
		if !hasBinding(short, b) {
			t.Errorf("list short help omits %s", name)
		}
	}
}

func TestDetailFullHelp(t *testing.T) {
	k := defaultKeys()
	full := flatten(k.help(modeDetail).FullHelp())
	for name, b := range map[string]key.Binding{
		"Follow": k.Follow, "Search": k.Search, "NextMember": k.NextMember,
		"OpenLog": k.OpenLog, "Stop": k.Stop,
	} {
		if !hasBinding(full, b) {
			t.Errorf("detail full help omits %s", name)
		}
	}
}

func TestConfirmShortHelpIsYesNo(t *testing.T) {
	k := defaultKeys()
	short := k.help(modeConfirm).ShortHelp()
	if len(short) != 2 || !hasBinding(short[:1], k.Yes) || !hasBinding(short[1:], k.No) {
		t.Fatalf("confirm short help = %+v, want exactly Yes, No", short)
	}
}

func TestEveryModeHasHelp(t *testing.T) {
	k := defaultKeys()
	for _, m := range []mode{modeList, modeFilter, modeDetail, modeSearch, modeConfirm} {
		h := k.help(m)
		if len(h.ShortHelp()) == 0 || len(h.FullHelp()) == 0 {
			t.Errorf("mode %d has empty help", m)
		}
	}
}

func TestListFullHelpHasPageKeys(t *testing.T) {
	k := defaultKeys()
	full := flatten(k.help(modeList).FullHelp())
	for name, b := range map[string]key.Binding{"NextPage": k.NextPage, "PrevPage": k.PrevPage} {
		if !hasBinding(full, b) {
			t.Errorf("list full help omits %s", name)
		}
	}
}
