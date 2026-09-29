package dashboard

import "testing"

func TestComputeLayoutWideShowsPreview(t *testing.T) {
	l := computeLayout(200, 50)
	if !l.ShowPreview {
		t.Fatal("200 cols: preview hidden")
	}
	if l.ListW+l.PreviewW+1 != 200 {
		t.Fatalf("ListW %d + PreviewW %d + 1 != 200", l.ListW, l.PreviewW)
	}
	if l.ListW != 110 {
		t.Fatalf("ListW = %d, want 55%% of 200 = 110", l.ListW)
	}
	if l.Compact {
		t.Fatal("50 rows: header collapsed")
	}
}

func TestComputeLayoutPreviewThreshold(t *testing.T) {
	if l := computeLayout(119, 50); l.ShowPreview || l.ListW != 119 || l.PreviewW != 0 {
		t.Fatalf("119 cols: %+v, want no preview and a full-width list", l)
	}
	l := computeLayout(120, 50)
	if !l.ShowPreview {
		t.Fatal("120 cols: preview hidden")
	}
	if l.ListW < minWidth {
		t.Fatalf("120 cols: ListW %d below the %d minimum", l.ListW, minWidth)
	}
	if l.ListW+l.PreviewW+1 != 120 {
		t.Fatalf("120 cols: ListW %d + PreviewW %d + 1 != 120", l.ListW, l.PreviewW)
	}
}

func TestComputeLayoutCompact(t *testing.T) {
	l := computeLayout(120, 29)
	if !l.Compact || l.HeaderH != 1 {
		t.Fatalf("120×29: %+v, want Compact with HeaderH 1", l)
	}
}

func TestComputeLayoutTooSmall(t *testing.T) {
	for _, sz := range [][2]int{{59, 30}, {80, 15}} {
		if l := computeLayout(sz[0], sz[1]); !l.TooSmall {
			t.Fatalf("%d×%d: not TooSmall", sz[0], sz[1])
		}
	}
	if l := computeLayout(60, 16); l.TooSmall {
		t.Fatal("60×16 is the minimum and must fit")
	}
}

func TestComputeLayoutRowsAddUp(t *testing.T) {
	for _, w := range []int{60, 90, 119, 120, 200} {
		for _, h := range []int{16, 24, 29, 30, 50} {
			l := computeLayout(w, h)
			if l.TooSmall {
				continue
			}
			if got := l.HeaderH + 1 + l.BodyH + 1; got != h {
				t.Fatalf("%d×%d: HeaderH %d + tab bar + BodyH %d + help = %d", w, h, l.HeaderH, l.BodyH, got)
			}
		}
	}
}

// At 120-145 cols a bare 55% split left the list under its fixed columns and
// silently dropped EFF and PROJECT; the split must keep them.
func TestSplitListKeepsEveryFixedColumn(t *testing.T) {
	for _, w := range []int{120, 130, 145, 200} {
		l := computeLayout(w, 40)
		if !l.ShowPreview {
			t.Fatalf("width %d: want preview", w)
		}
		c := layoutColumns(l.ListW - 2)
		if c.Effort == 0 || c.Project == 0 {
			t.Fatalf("width %d: list inner %d drops columns: %+v", w, l.ListW-2, c)
		}
		if l.PreviewW < 40 {
			t.Fatalf("width %d: preview only %d cols", w, l.PreviewW)
		}
	}
}
