package dashboard

// Terminal size limits. Below minWidth×minHeight the frame cannot hold the
// header, tab bar, one row and help, so a notice replaces it.
const (
	minWidth        = 60
	minHeight       = 16
	previewMinWidth = 120
)

// layout is the frame geometry, computed once per WindowSizeMsg. View and
// Update both read it: if they sized the body differently, the viewport would
// render a frame the view then clips, leaving stale rows on screen.
type layout struct {
	Width, Height, HeaderH, BodyH, ListW, PreviewW int
	ShowPreview, Compact, TooSmall                 bool
}

// computeLayout splits the terminal into header, tab bar, body and help bar:
// HeaderH + 1 + BodyH + 1 == Height. From previewMinWidth up the body splits
// into the list (55%, but never narrower than its fixed columns plus the
// border) and the preview, with a 1-col gap
// between the two bordered boxes: ListW + 1 + PreviewW == Width. Below it the
// list takes the full width.
func computeLayout(width, height int) layout {
	l := layout{Width: width, Height: height, ListW: width}
	l.TooSmall = width < minWidth || height < minHeight
	if width >= previewMinWidth {
		l.ShowPreview = true
		// A 55% split alone leaves the list under its fixed columns at 120-145
		// cols, which silently dropped EFF and PROJECT.
		l.ListW = max(listFixedWidth+2, width*55/100)
		l.PreviewW = width - l.ListW - 1
	}
	l.Compact = height < compactHeaderBelowHeight
	l.HeaderH = len(bannerLines)
	if l.Compact {
		l.HeaderH = 1
	}
	l.BodyH = max(0, height-l.HeaderH-2)
	return l
}
