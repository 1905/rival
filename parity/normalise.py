"""Normalisation and comparison for the parity runner.

Volatile values become markers so expectations can be written from the Go
source:

* temp paths -> ``<HOME>``, ``<ROOT>``, ``<BIN>`` (longest path first);
* UUIDs -> ``<UUID1>``, ``<UUID2>``... in first-seen order; one value keeps one
  marker, so distinct sessions stay distinct;
* PIDs -> ``<PID1>``... the same way, but only where the key or text says it is
  a PID (JSON keys ``pid``/``owner_pid``, text ``pid=N``); ``pid_start`` and
  ``owner_pid_start`` -> ``<PIDSTART1>``...;
* RFC 3339 timestamps -> ``<TIME>``; JSON key ``duration`` -> ``<DURATION>``.
  Times carry no index: Go writes them at second precision, so whether two
  of them are equal depends on scheduling.

Every other number (exit codes, ratings, line numbers, byte and line
counts) stays exact. Scenario ``normalise`` rules add patterns; nothing
matches any string by default. A ``uuid_prefix`` rule maps a printed UUID
prefix (``id[:8]``) to the marker of the one full UUID already seen that
starts with it, e.g. ``<UUID1:8>``; an unknown or ambiguous prefix raises
NormaliseError.
"""

from __future__ import annotations

import json
import re

UUID_RE = re.compile(r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b")
TIME_RE = re.compile(r"\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})")
PID_TEXT_RE = re.compile(r"\bpid=(\d+)")

PID_KEYS = frozenset({"pid", "owner_pid"})
PIDSTART_KEYS = frozenset({"pid_start", "owner_pid_start"})
DURATION_KEYS = frozenset({"duration"})

RULE_KINDS = ("pid", "duration", "replace", "uuid_prefix")
LOG_LEVELS = frozenset({"trace", "debug", "info", "warn", "error", "fatal", "panic"})


class NormaliseError(Exception):
    """A rule cannot map its match, for example an unknown UUID prefix."""


def validate_rules(rules) -> list:
    if not isinstance(rules, list):
        return ["normalise: want a list"]
    problems = []
    for i, rule in enumerate(rules):
        where = "normalise[%d]" % i
        if not isinstance(rule, dict) or "regex" not in rule or "kind" not in rule:
            problems.append(where + ": want {regex, kind[, replace]}")
            continue
        if rule["kind"] not in RULE_KINDS:
            problems.append("%s.kind: one of %s" % (where, RULE_KINDS))
        allowed = {"regex", "kind", "replace"} if rule["kind"] == "replace" else {"regex", "kind"}
        if set(rule) != allowed:
            problems.append("%s: keys must be %s" % (where, sorted(allowed)))
        try:
            if re.compile(rule["regex"]).groups != 1:
                problems.append(where + ".regex: want exactly one group")
        except (re.error, TypeError) as err:
            problems.append("%s.regex: %s" % (where, err))
    return problems


class Normaliser:
    """One per scenario: markers stay consistent across every output."""

    def __init__(self, paths, rules=()):
        pairs = {p: m for p, m in paths if p}
        self.paths = sorted(pairs.items(), key=lambda pm: len(pm[0]), reverse=True)
        self.rules = [(re.compile(r["regex"]), r) for r in rules]
        self.markers = {}

    def marker(self, kind: str, value) -> str:
        table = self.markers.setdefault(kind, {})
        if value not in table:
            table[value] = "<%s%d>" % (kind, len(table) + 1)
        return table[value]

    def uuid_prefix(self, prefix: str) -> str:
        """Marker of the one already-seen UUID starting with prefix, suffixed with its length."""
        seen = [(u, mark) for u, mark in self.markers.get("UUID", {}).items() if prefix and u.startswith(prefix)]
        if len(seen) != 1:
            raise NormaliseError("UUID prefix %r matches %d seen UUIDs, want 1" % (prefix, len(seen)))
        return "%s:%d>" % (seen[0][1][:-1], len(prefix))

    def text(self, s: str) -> str:
        for path, mark in self.paths:
            s = s.replace(path, mark)
        s = UUID_RE.sub(lambda m: self.marker("UUID", m.group(0)), s)
        s = TIME_RE.sub("<TIME>", s)
        s = PID_TEXT_RE.sub(lambda m: "pid=" + self.marker("PID", int(m.group(1))), s)
        for regex, rule in self.rules:
            s = regex.sub(lambda m, rule=rule: self._apply_rule(m, rule), s)
        return s

    def _apply_rule(self, m, rule) -> str:
        if rule["kind"] == "pid":
            rep = self.marker("PID", int(m.group(1)))
        elif rule["kind"] == "duration":
            rep = "<DURATION>"
        elif rule["kind"] == "uuid_prefix":
            rep = self.uuid_prefix(m.group(1))
        else:
            rep = rule["replace"]
        whole, start = m.group(0), m.start(0)
        return whole[: m.start(1) - start] + rep + whole[m.end(1) - start:]

    def json(self, value, key=None):
        if isinstance(value, dict):
            return {k: self.json(v, k) for k, v in value.items()}
        if isinstance(value, list):
            return [self.json(v) for v in value]
        if type(value) is int and value != 0:
            if key in PID_KEYS:
                return self.marker("PID", value)
            if key in PIDSTART_KEYS:
                return self.marker("PIDSTART", value)
        if isinstance(value, str):
            if key in DURATION_KEYS and value:
                return "<DURATION>"
            return self.text(value)
        return value


def split_stderr(text: str):
    """Splits stderr into plain lines and JSON log objects, in order."""
    if not text:
        return [], []
    lines = text.split("\n")
    if text.endswith("\n"):
        lines.pop()
    plain, events = [], []
    for line in lines:
        if line.startswith("{"):
            try:
                obj = json.loads(line)
            except ValueError:
                obj = None
            if isinstance(obj, dict):
                events.append(obj)
                continue
        plain.append(line)
    return plain, events


def event_problems(event: dict) -> list:
    """zerolog shape: a known level, app=rival and an RFC 3339 time."""
    problems = []
    if event.get("level") not in LOG_LEVELS:
        problems.append("level %r" % event.get("level"))
    if event.get("app") != "rival":
        problems.append("app %r" % event.get("app"))
    t = event.get("time")
    if not isinstance(t, str) or not TIME_RE.fullmatch(t):
        problems.append("time %r" % t)
    return problems


def matches(expected, actual) -> bool:
    """Exact for scalars and lists; dicts are subsets; {"$regex": r} full-matches a string."""
    if isinstance(expected, dict):
        if set(expected) == {"$regex"}:
            return isinstance(actual, str) and re.fullmatch(expected["$regex"], actual) is not None
        return isinstance(actual, dict) and all(
            k in actual and matches(v, actual[k]) for k, v in expected.items()
        )
    if isinstance(expected, list):
        return (
            isinstance(actual, list)
            and len(expected) == len(actual)
            and all(matches(e, a) for e, a in zip(expected, actual))
        )
    return type(expected) is type(actual) and expected == actual


def validate_expected_events(events) -> list:
    if not isinstance(events, list):
        return ["log_events: want a list"]
    problems = []
    for i, e in enumerate(events):
        if not isinstance(e, dict) or not {"level", "message"} <= set(e) or set(e) - {"level", "message", "fields"}:
            problems.append("log_events[%d]: want {level, message[, fields]}" % i)
        elif not isinstance(e.get("fields", {}), dict):
            problems.append("log_events[%d].fields: want an object" % i)
    return problems


def event_matches(expected: dict, actual: dict) -> bool:
    # zerolog omits an empty message.
    return (
        actual.get("level") == expected["level"]
        and actual.get("message", "") == expected["message"]
        and matches(expected.get("fields", {}), actual)
    )


def match_events(expected: list, actual: list):
    """Multiset match. Returns (unmatched expected, unmatched actual)."""
    owner = [None] * len(actual)

    def assign(e, seen):
        for a in range(len(actual)):
            if a not in seen and event_matches(expected[e], actual[a]):
                seen.add(a)
                if owner[a] is None or assign(owner[a], seen):
                    owner[a] = e
                    return True
        return False

    missing = [expected[e] for e in range(len(expected)) if not assign(e, set())]
    extra = [actual[a] for a in range(len(actual)) if owner[a] is None]
    return missing, extra
