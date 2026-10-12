"""Only the intentional formula deprecation may pass the release gate."""

import unittest

from check_goreleaser import accepted_check


class CheckTest(unittest.TestCase):
    def test_valid_config(self):
        self.assertTrue(accepted_check(0, "configuration validated"))

    def test_formula_deprecation(self):
        self.assertTrue(accepted_check(2, "  • DEPRECATED:  brews  should not be used anymore"))

    def test_invalid_config_is_never_accepted(self):
        for code in (1, 3, -9):
            self.assertFalse(accepted_check(code, "DEPRECATED: brews should not be used anymore"))

    def test_unknown_or_missing_deprecations_fail(self):
        for output in ("", "DEPRECATED: another", "DEPRECATED: brews\nDEPRECATED: another"):
            self.assertFalse(accepted_check(2, output))


if __name__ == "__main__":
    unittest.main()
