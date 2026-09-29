// Package sessionview derives display data from stored sessions: how a group
// is bucketed, labelled, and timed. The TUI reads through it, and the Mac app
// ports the same rules. It sits between internal/session (file parsing) and
// the front end, and never mutates the sessions it receives.
package sessionview

import (
	"time"

	"github.com/1905/rival/internal/session"
)

// Bucket is one row in a dashboard: either a multi-session group or a single
// standalone session.
type Bucket struct {
	// Key identifies the bucket. It holds the GroupID for a group, and
	// "solo:<SessionID>" for a standalone session. Use it for selection and
	// anchors. Never use Sessions[0].GroupID, which is empty for a solo
	// session and therefore collides across all of them.
	Key string
	// Sessions is in SortGroupMembers order. sessionview never mutates them.
	Sessions []*session.Session
}

// Group buckets sessions by GroupID, preserving the order in which each key
// first appears. A session with no GroupID becomes its own bucket.
func Group(sessions []*session.Session) []Bucket {
	buckets := make(map[string][]*session.Session)
	var order []string

	for _, s := range sessions {
		key := s.GroupID
		if key == "" {
			key = "solo:" + s.ID
		}
		if _, ok := buckets[key]; !ok {
			order = append(order, key)
		}
		buckets[key] = append(buckets[key], s)
	}

	result := make([]Bucket, 0, len(order))
	for _, key := range order {
		members := buckets[key]
		session.SortGroupMembers(members)
		result = append(result, Bucket{Key: key, Sessions: members})
	}
	return result
}

// Status reduces the members to one status. Tier: running > queued > failed >
// completed.
func Status(sessions []*session.Session) string {
	for _, s := range sessions {
		if s.Status == "running" {
			return "running"
		}
	}
	for _, s := range sessions {
		if s.Status == "queued" {
			return "queued"
		}
	}
	for _, s := range sessions {
		if s.Status == "failed" {
			return "failed"
		}
	}
	return "completed"
}

// Kind classifies a GROUP, and returns exactly one of "security", "antislop",
// "plan", or "megareview". There is no empty value. A solo row must display the
// session's own Mode instead of calling this.
//
// Precedence: security, then antislop, then plan, else megareview. A group never mixes antislop and plan members, because one
// command creates all of them.
func Kind(sessions []*session.Session) string {
	for _, s := range sessions {
		if s.Mode == session.ModeSecurity {
			return "security"
		}
	}
	for _, s := range sessions {
		if s.Mode == "antislop" {
			return "antislop"
		}
	}
	for _, s := range sessions {
		if s.Mode == "plan" {
			return "plan"
		}
	}
	return "megareview"
}

// Effort returns the shared effort of the members, or "mixed" when they
// differ.
func Effort(sessions []*session.Session) string {
	if len(sessions) == 0 {
		return ""
	}
	effort := sessions[0].Effort
	for _, s := range sessions[1:] {
		if s.Effort != effort {
			return "mixed"
		}
	}
	return effort
}

// Elapsed measures the wall-clock span of the group: from the earliest member
// start to the latest member end. A member that is running or queued extends
// the span to now. A queued member counts from QueuedAt. It returns "-" when
// no member has started.
func Elapsed(sessions []*session.Session) string {
	now := time.Now()
	var earliest, latest time.Time
	for _, s := range sessions {
		start := s.StartTime
		if s.QueuedAt != nil && (start.IsZero() || s.QueuedAt.Before(start)) {
			start = *s.QueuedAt
		}
		if start.IsZero() {
			continue
		}

		end := start
		switch {
		case s.Status == "running" || s.Status == "queued":
			end = now
		case s.EndTime != nil:
			end = *s.EndTime
		case s.Duration != "":
			if duration, err := time.ParseDuration(s.Duration); err == nil {
				end = start.Add(duration)
			}
		}
		if end.Before(start) {
			end = start
		}
		if earliest.IsZero() || start.Before(earliest) {
			earliest = start
		}
		if latest.IsZero() || end.After(latest) {
			latest = end
		}
	}
	if !earliest.IsZero() && latest.After(earliest) {
		return latest.Sub(earliest).Round(time.Second).String()
	}
	return "-"
}
