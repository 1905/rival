"""Unit tests for e2e/normalise.py. Run: python3 -m unittest discover -s e2e -p 'test_*.py'"""

import unittest

import normalise

U1 = "0b7c6a43-5d1e-4c2f-9a8b-1c2d3e4f5a6b"
U2 = "f1e2d3c4-b5a6-4978-8a9b-0c1d2e3f4a5b"


class NormaliserTest(unittest.TestCase):
    def norm(self, rules=()):
        return normalise.Normaliser([("/tmp/r/home", "<HOME>"), ("/tmp/r", "<ROOT>"), ("/x/rival", "<BIN>")], rules)

    def test_uuid_identity(self):
        n = self.norm()
        self.assertEqual(n.text("%s %s %s" % (U1, U2, U1)), "<UUID1> <UUID2> <UUID1>")
        self.assertEqual(n.json({"id": U2, "group_id": U1}), {"id": "<UUID2>", "group_id": "<UUID1>"})

    def test_paths_longest_first(self):
        n = self.norm()
        self.assertEqual(n.text("/tmp/r/home/.rival /tmp/r/work /x/rival"), "<HOME>/.rival <ROOT>/work <BIN>")

    def test_pid_by_key_and_text_only(self):
        n = self.norm()
        self.assertEqual(n.text("rival: detached pid=4242"), "rival: detached pid=<PID1>")
        got = n.json({"pid": 4242, "owner_pid": 77, "pid_start": 9, "owner_pid_start": 9,
                      "exit_code": 3, "output_bytes": 4242, "output_lines": 12, "rating": 7, "line": 88})
        self.assertEqual(got, {"pid": "<PID1>", "owner_pid": "<PID2>", "pid_start": "<PIDSTART1>",
                               "owner_pid_start": "<PIDSTART1>", "exit_code": 3, "output_bytes": 4242,
                               "output_lines": 12, "rating": 7, "line": 88})
        self.assertEqual(n.text("exit 4242 at line 77"), "exit 4242 at line 77")

    def test_zero_pid_stays(self):
        self.assertEqual(self.norm().json({"pid": 0, "owner_pid": 0}), {"pid": 0, "owner_pid": 0})

    def test_times_and_duration(self):
        n = self.norm()
        got = n.json({"start_time": "2026-10-02T11:00:00.123456789+02:00", "end_time": "2026-10-02T09:00:01Z",
                      "duration": "1.5s", "error": "at 2026-10-02T09:00:01Z"})
        self.assertEqual(got, {"start_time": "<TIME>", "end_time": "<TIME>", "duration": "<DURATION>",
                               "error": "at <TIME>"})
        self.assertEqual(n.json({"duration": ""}), {"duration": ""})

    def test_rules(self):
        rules = [{"regex": r"^\S+\s+\S+\s+\S+\s+(\d+)", "kind": "pid"},
                 {"regex": r"(\d+s)\s+/", "kind": "duration"},
                 {"regex": r"v(\d+\.\d+\.\d+)", "kind": "replace", "replace": "<V>"}]
        self.assertEqual(normalise.validate_rules(rules), [])
        n = self.norm(rules)
        self.assertEqual(n.text("#1  waiting  review  5150  3s  /w v1.2.3"),
                         "#1  waiting  review  <PID1>  <DURATION>  /w v<V>")

    PREFIX_RULE = {"regex": r"(?m)^([0-9a-f]{8}) (?:completed|failed) exit=", "kind": "uuid_prefix"}

    def test_uuid_prefix_links_to_seen_uuid(self):
        self.assertEqual(normalise.validate_rules([self.PREFIX_RULE]), [])
        n = self.norm([self.PREFIX_RULE])
        self.assertEqual(n.json({"session": U1}), {"session": "<UUID1>"})
        n.text(U2)
        self.assertEqual(n.text("%s failed exit=1 3s\n%s completed exit=0 1s\n" % (U2[:8], U1[:8])),
                         "<UUID2:8> failed exit=1 3s\n<UUID1:8> completed exit=0 1s\n")
        # The full UUID is replaced first, so the rule never sees it.
        self.assertEqual(n.text("%s completed exit=0" % U1), "<UUID1> completed exit=0")

    def test_uuid_prefix_rejects_unknown_and_ambiguous(self):
        n = self.norm([self.PREFIX_RULE])
        with self.assertRaisesRegex(normalise.NormaliseError, "'0b7c6a43' matches 0 seen UUIDs"):
            n.text("%s completed exit=0" % U1[:8])
        n.text("%s %s" % (U1, U1[:9] + "0000-4000-8000-000000000000"))
        with self.assertRaisesRegex(normalise.NormaliseError, "matches 2 seen UUIDs"):
            n.text("%s completed exit=0" % U1[:8])
        self.assertNotIsInstance(normalise.NormaliseError("x"), ValueError)

    def test_rule_validation(self):
        self.assertTrue(normalise.validate_rules([{"regex": "(a)(b)", "kind": "pid"}]))
        self.assertTrue(normalise.validate_rules([{"regex": "(a)", "kind": "any"}]))
        self.assertTrue(normalise.validate_rules([{"regex": "(a)", "kind": "pid", "replace": "x"}]))


class StderrTest(unittest.TestCase):
    def test_split(self):
        text = ('{"level":"info","app":"rival","time":"2026-10-02T09:00:00Z","message":"m"}\n'
                "rival: detached pid=1\n\n{not json\n[1]\n")
        plain, events = normalise.split_stderr(text)
        self.assertEqual(plain, ["rival: detached pid=1", "", "{not json", "[1]"])
        self.assertEqual(events, [{"level": "info", "app": "rival", "time": "2026-10-02T09:00:00Z", "message": "m"}])
        self.assertEqual(normalise.split_stderr(""), ([], []))
        self.assertEqual(normalise.split_stderr("no newline"), (["no newline"], []))

    def test_update_notice_blank_lines(self):
        # The update check prints "\n  Update available: ...\n\n".
        plain, _ = normalise.split_stderr("\n  Update available: vdev → v9.9.9 — run 'rival update'\n\n")
        self.assertEqual(plain, ["", "  Update available: vdev → v9.9.9 — run 'rival update'", ""])

    def test_event_shape(self):
        ok = {"level": "warn", "app": "rival", "time": "2026-10-02T09:00:00+02:00"}
        self.assertEqual(normalise.event_problems(ok), [])
        self.assertEqual(len(normalise.event_problems({"level": "loud", "app": "x"})), 3)


class EventMatchTest(unittest.TestCase):
    def ev(self, msg, **fields):
        return dict({"level": "info", "app": "rival", "time": "<TIME>", "message": msg}, **fields)

    def test_multiset_order_insensitive(self):
        actual = [self.ev("b", session="<UUID2>"), self.ev("a", session="<UUID1>", pid="<PID1>")]
        expected = [{"level": "info", "message": "a", "fields": {"session": "<UUID1>"}},
                    {"level": "info", "message": "b", "fields": {"session": "<UUID2>"}}]
        self.assertEqual(normalise.match_events(expected, actual), ([], []))

    def test_unexpected_and_missing(self):
        actual = [self.ev("a"), self.ev("a")]
        missing, extra = normalise.match_events([{"level": "info", "message": "a"}], actual)
        self.assertEqual((missing, len(extra)), ([], 1))
        missing, extra = normalise.match_events([{"level": "error", "message": "a"}], [self.ev("a")])
        self.assertEqual((len(missing), len(extra)), (1, 1))

    def test_augmenting_assignment(self):
        # A greedy first fit would give the general expectation the specific event.
        actual = [self.ev("m", session="<UUID1>"), self.ev("m", session="<UUID2>")]
        expected = [{"level": "info", "message": "m"},
                    {"level": "info", "message": "m", "fields": {"session": "<UUID1>"}}]
        self.assertEqual(normalise.match_events(expected, actual), ([], []))

    def test_empty_message_omitted(self):
        self.assertTrue(normalise.event_matches({"level": "info", "message": ""}, {"level": "info"}))

    def test_matches_is_typed(self):
        self.assertFalse(normalise.matches(1, True))
        self.assertFalse(normalise.matches(1, 1.0))
        self.assertTrue(normalise.matches({"a": {"$regex": "x+"}}, {"a": "xxx", "b": 2}))
        self.assertFalse(normalise.matches([1], [1, 2]))

    def test_expected_event_schema(self):
        self.assertEqual(normalise.validate_expected_events([{"level": "info", "message": "m"}]), [])
        self.assertTrue(normalise.validate_expected_events([{"level": "info"}]))
        self.assertTrue(normalise.validate_expected_events([{"level": "info", "message": "m", "extra": 1}]))


if __name__ == "__main__":
    unittest.main()
