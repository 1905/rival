//! Dashboard buckets and the group reducers.
//!
//! Go: `internal/sessionview/group.go`.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Local, TimeDelta};

use crate::gojson;
use crate::gostd;
use crate::session::{self, Session};

/// One row in a dashboard: either a multi-session group or a single
/// standalone session.
#[derive(Debug, Clone)]
pub struct Bucket {
    /// Identifies the bucket. It holds the `group_id` for a group, and
    /// `"solo:<id>"` for a standalone session. Use it for selection and
    /// anchors. Never use `sessions[0].group_id`, which is empty for a solo
    /// session and therefore collides across all of them.
    pub key: String,
    /// In [`session::sort_group_members`] order. The sessions are shared,
    /// never mutated.
    pub sessions: Vec<Arc<Session>>,
}

/// Buckets sessions by `group_id`, preserving the order in which each key
/// first appears. A session with no `group_id` becomes its own bucket. The
/// input slice keeps its order.
pub fn group(sessions: &[Arc<Session>]) -> Vec<Bucket> {
    let mut buckets: HashMap<String, Vec<Arc<Session>>> = HashMap::new();
    let mut order = Vec::new();

    for s in sessions {
        let key = if s.group_id.is_empty() {
            format!("solo:{}", s.id)
        } else {
            s.group_id.clone()
        };
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(Arc::clone(s));
    }

    order
        .into_iter()
        .map(|key| {
            let mut members = buckets.remove(&key).expect("key recorded in order");
            session::sort_group_members(&mut members);
            Bucket {
                key,
                sessions: members,
            }
        })
        .collect()
}

/// Reduces the members to one status. Tier: running > queued > failed >
/// completed.
pub fn status<S: Borrow<Session>>(sessions: &[S]) -> &'static str {
    for tier in ["running", "queued", "failed"] {
        if sessions.iter().any(|s| s.borrow().status == tier) {
            return tier;
        }
    }
    "completed"
}

/// Classifies a GROUP, and returns exactly one of "security", "plan", or
/// "megareview". There is no empty value. A solo row must display the
/// session's own mode instead of calling this.
///
/// Precedence: security, then plan, else megareview.
pub fn kind<S: Borrow<Session>>(sessions: &[S]) -> &'static str {
    for mode in [session::MODE_SECURITY, session::MODE_PLAN] {
        if sessions.iter().any(|s| s.borrow().mode == mode) {
            return mode;
        }
    }
    "megareview"
}

/// Returns the shared effort of the members, or "mixed" when they differ.
pub fn effort<S: Borrow<Session>>(sessions: &[S]) -> String {
    let Some((first, rest)) = sessions.split_first() else {
        return String::new();
    };
    let effort = &first.borrow().effort;
    if rest.iter().any(|s| &s.borrow().effort != effort) {
        return "mixed".to_string();
    }
    effort.clone()
}

/// [`elapsed_at`] with the current wall clock.
pub fn elapsed<S: Borrow<Session>>(sessions: &[S]) -> String {
    elapsed_at(sessions, Local::now().fixed_offset())
}

/// Measures the wall-clock span of the group: from the earliest member
/// start to the latest member end. A member that is running or queued
/// extends the span to `now`. A queued member counts from `queued_at`. It
/// returns "-" when no member has started.
pub fn elapsed_at<S: Borrow<Session>>(sessions: &[S], now: DateTime<FixedOffset>) -> String {
    // Go uses the zero time.Time as the "unset" sentinel for both bounds.
    let zero = gojson::zero_time();
    let (mut earliest, mut latest) = (zero, zero);
    for s in sessions {
        let s = s.borrow();
        let mut start = s.start_time;
        if let Some(queued_at) = s.queued_at
            && (start == zero || queued_at < start)
        {
            start = queued_at;
        }
        if start == zero {
            continue;
        }

        let mut end = start;
        if s.status == "running" || s.status == "queued" {
            end = now;
        } else if let Some(end_time) = s.end_time {
            end = end_time;
        } else if !s.duration.is_empty()
            && let Ok(nanos) = gostd::parse_duration(&s.duration)
        {
            end = start
                .checked_add_signed(TimeDelta::nanoseconds(nanos))
                .unwrap_or(start);
        }
        if end < start {
            end = start;
        }
        if earliest == zero || start < earliest {
            earliest = start;
        }
        if latest == zero || end > latest {
            latest = end;
        }
    }
    if earliest != zero && latest > earliest {
        return session::duration_text(session::sub_nanos(latest, earliest));
    }
    "-".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CLAUDE_MODEL as CLAUDE, GPT56_SOL_MODEL as SOL};
    use chrono::TimeZone;

    fn sess(
        id: &str,
        group_id: &str,
        status: &str,
        mode: &str,
        cli: &str,
        model: &str,
        effort: &str,
    ) -> Arc<Session> {
        Arc::new(Session {
            id: id.into(),
            group_id: group_id.into(),
            status: status.into(),
            mode: mode.into(),
            cli: cli.into(),
            model: model.into(),
            effort: effort.into(),
            ..Session::default()
        })
    }

    fn base() -> DateTime<FixedOffset> {
        FixedOffset::east_opt(3 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 10, 3, 12, 0, 0)
            .unwrap()
    }

    fn minutes(n: i64) -> TimeDelta {
        TimeDelta::minutes(n)
    }

    // Go: TestGroupBucketsAndKeys.
    #[test]
    fn group_buckets_and_keys() {
        let solo = sess("s1", "", "completed", "review", "codex", SOL, "high");
        let a = sess("a1", "grp", "completed", "plan", "codex", SOL, "xhigh");
        let b = sess("b1", "grp", "completed", "plan", "claude", CLAUDE, "xhigh");

        let buckets = group(&[solo, a, b]);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].key, "solo:s1");
        assert_eq!(buckets[1].key, "grp");
        assert_eq!(buckets[1].sessions.len(), 2);
    }

    // Go: TestGroupPreservesFirstAppearanceOrder.
    #[test]
    fn group_preserves_first_appearance_order() {
        let first = sess("x", "g2", "completed", "review", "codex", SOL, "high");
        let second = sess("y", "g1", "completed", "review", "codex", SOL, "high");
        let third = sess("z", "g2", "completed", "review", "claude", CLAUDE, "high");

        let buckets = group(&[first, second, third]);
        let keys: Vec<_> = buckets.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(keys, ["g2", "g1"]);
    }

    // Go: TestGroupDoesNotMutateInput. Also checks the input order, which a
    // Rust caller could otherwise lose to an in-place member sort.
    #[test]
    fn group_does_not_mutate_input() {
        // Input order is the reverse of the sorted member order (sol before
        // claude), so an in-place sort would show.
        let a = sess("a", "g", "completed", "plan", "claude", CLAUDE, "high");
        let b = sess("b", "g", "running", "plan", "codex", SOL, "low");
        let input = vec![Arc::clone(&a), Arc::clone(&b)];
        let before: Vec<Session> = input.iter().map(|s| (**s).clone()).collect();

        let buckets = group(&input);

        assert!(Arc::ptr_eq(&input[0], &a) && Arc::ptr_eq(&input[1], &b));
        let after: Vec<Session> = input.iter().map(|s| (**s).clone()).collect();
        assert_eq!(after, before, "group mutated its input sessions");
        let members: Vec<_> = buckets[0].sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            members,
            ["b", "a"],
            "members not in sort_group_members order"
        );
    }

    // Go: TestStatusTier.
    #[test]
    fn status_tier() {
        let cases: [(&str, &[&str], &str); 4] = [
            (
                "running wins",
                &["completed", "running", "queued"],
                "running",
            ),
            (
                "queued beats failed",
                &["failed", "queued", "completed"],
                "queued",
            ),
            ("failed beats completed", &["completed", "failed"], "failed"),
            ("all completed", &["completed", "completed"], "completed"),
        ];
        for (name, statuses, want) in cases {
            let sessions: Vec<_> = statuses
                .iter()
                .enumerate()
                .map(|(i, st)| {
                    let id = char::from(b'a' + i as u8).to_string();
                    sess(&id, "g", st, "review", "codex", SOL, "high")
                })
                .collect();
            assert_eq!(status(&sessions), want, "{name}");
        }
    }

    // Go: TestKindPrecedence.
    #[test]
    fn kind_precedence() {
        let cases: [(&str, &[&str], &str); 3] = [
            ("plan", &["plan", "plan"], "plan"),
            ("review is megareview", &["review", "review"], "megareview"),
            ("raw is megareview", &["raw"], "megareview"),
        ];
        for (name, modes, want) in cases {
            let sessions: Vec<_> = modes
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let id = char::from(b'a' + i as u8).to_string();
                    sess(&id, "g", "completed", m, "codex", SOL, "high")
                })
                .collect();
            assert_eq!(kind(&sessions), want, "{name}");
        }
    }

    // Not in the Go suite: security precedes every other kind.
    #[test]
    fn kind_security_wins() {
        let sessions = [
            sess("a", "g", "completed", "plan", "codex", SOL, "high"),
            sess("b", "g", "completed", "security", "codex", SOL, "high"),
        ];
        assert_eq!(kind(&sessions), "security");
    }

    // Go: TestEffort.
    #[test]
    fn effort_shared_mixed_and_empty() {
        let same = [
            sess("a", "g", "completed", "plan", "codex", SOL, "xhigh"),
            sess("b", "g", "completed", "plan", "claude", CLAUDE, "xhigh"),
        ];
        assert_eq!(effort(&same), "xhigh");

        let mixed = [
            sess("a", "g", "completed", "plan", "codex", SOL, "xhigh"),
            sess("b", "g", "completed", "plan", "claude", CLAUDE, "low"),
        ];
        assert_eq!(effort(&mixed), "mixed");

        assert_eq!(effort::<Session>(&[]), "");
    }

    // Go: TestElapsedSpansTheWholeGroup. Elapsed is the wall-clock span of
    // the whole group; the TUI used to report the longest single member.
    #[test]
    fn elapsed_spans_the_whole_group() {
        let base = base();
        let now = base + minutes(30);
        let sequential = [
            Session {
                id: "a".into(),
                status: "completed".into(),
                start_time: base,
                end_time: Some(base + minutes(4)),
                ..Session::default()
            },
            Session {
                id: "b".into(),
                status: "completed".into(),
                start_time: base + minutes(4),
                end_time: Some(base + minutes(7)),
                ..Session::default()
            },
        ];
        assert_eq!(
            elapsed_at(&sequential, now),
            "7m0s",
            "span, not the 4m longest member"
        );

        let overlapping = [
            Session {
                id: "a".into(),
                status: "completed".into(),
                start_time: base,
                end_time: Some(base + minutes(10)),
                ..Session::default()
            },
            Session {
                id: "b".into(),
                status: "completed".into(),
                start_time: base + minutes(2),
                end_time: Some(base + minutes(5)),
                ..Session::default()
            },
        ];
        assert_eq!(elapsed_at(&overlapping, now), "10m0s");
    }

    // Go: TestElapsedUsesDurationFallbackAndQueuedAt. The injected `now`
    // makes the queued span exact instead of Go's 9m..12m window.
    #[test]
    fn elapsed_uses_duration_fallback_and_queued_at() {
        let base = base();
        let with_duration = [Session {
            id: "a".into(),
            status: "completed".into(),
            start_time: base,
            duration: "3m0s".into(),
            ..Session::default()
        }];
        assert_eq!(elapsed_at(&with_duration, base + minutes(20)), "3m0s");

        let queued = [Session {
            id: "a".into(),
            status: "queued".into(),
            queued_at: Some(base),
            ..Session::default()
        }];
        assert_eq!(elapsed_at(&queued, base + minutes(10)), "10m0s");
    }

    // Go: TestElapsedWithoutStartIsDash.
    #[test]
    fn elapsed_without_start_is_dash() {
        let queued = [Session {
            id: "a".into(),
            status: "queued".into(),
            ..Session::default()
        }];
        assert_eq!(elapsed_at(&queued, base()), "-");
    }

    #[test]
    fn elapsed_edges_follow_go() {
        let base = base();
        // An earlier queued_at replaces start_time; running extends to now.
        let running = [Session {
            id: "a".into(),
            status: "running".into(),
            start_time: base + minutes(5),
            queued_at: Some(base),
            ..Session::default()
        }];
        assert_eq!(elapsed_at(&running, base + minutes(8)), "8m0s");

        // An end before the start clamps to the start: no span, so "-".
        let backwards = [Session {
            id: "a".into(),
            status: "completed".into(),
            start_time: base,
            end_time: Some(base - minutes(1)),
            ..Session::default()
        }];
        assert_eq!(elapsed_at(&backwards, base), "-");

        // An unparsable duration leaves end at start.
        let bad_duration = [Session {
            id: "a".into(),
            status: "completed".into(),
            start_time: base,
            duration: "soon".into(),
            ..Session::default()
        }];
        assert_eq!(elapsed_at(&bad_duration, base), "-");

        // Rounds to whole seconds, as Go's Round(time.Second).
        let fractional = [Session {
            id: "a".into(),
            status: "completed".into(),
            start_time: base,
            end_time: Some(base + TimeDelta::milliseconds(1500)),
            ..Session::default()
        }];
        assert_eq!(elapsed_at(&fractional, base), "2s");
    }
}
