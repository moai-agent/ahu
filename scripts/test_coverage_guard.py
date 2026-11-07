"""Release coverage thresholds must use exact counts and fail closed."""

import json
from pathlib import Path
import subprocess
import sys
import unittest


SCRIPT = Path(__file__).with_name("check-eval-mcp-coverage.py")


def report(eval_counts=(19, 20), mcp_counts=(19, 20), other_counts=(53, 60)):
    files = [
        {"filename": f"/fixture/src/{name}.rs", "summary": {
            "lines": {"covered": covered, "count": total}}}
        for name, (covered, total) in zip(
            ("eval", "mcp", "other"), (eval_counts, mcp_counts, other_counts))
    ]
    return {"data": [{"files": files, "totals": {"lines": {
        "covered": sum(row["summary"]["lines"]["covered"] for row in files),
        "count": sum(row["summary"]["lines"]["count"] for row in files),
    }}}]}


class CoverageGuardTests(unittest.TestCase):
    def check_report(self, data):
        return subprocess.run(
            [sys.executable, str(SCRIPT)], input=json.dumps(data),
            text=True, capture_output=True, check=False)

    def test_exact_group_thresholds_pass_above_workspace_threshold(self):
        result = self.check_report(report())
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_exact_ninety_percent_workspace_fails(self):
        self.assertEqual(self.check_report(report(other_counts=(52, 60))).returncode, 1)

    def test_workspace_rounding_does_not_turn_a_pass_into_failure(self):
        data = report((20, 20), (20, 20), (89961, 99961))
        result = self.check_report(data)
        self.assertIn("90.00%", result.stdout)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_group_rounding_does_not_turn_failure_into_a_pass(self):
        result = self.check_report(report((18999, 20000), (20, 20), (40000, 40000)))
        self.assertIn("required 95.00%", result.stdout)
        self.assertEqual(result.returncode, 1, result.stderr)

    def test_missing_or_invalid_counts_fail_closed(self):
        data = report()
        del data["data"][0]["totals"]
        self.assertEqual(self.check_report(data).returncode, 2)
        for invalid in ({"covered": 1, "count": 0}, {"covered": -1, "count": 2},
                        {"covered": True, "count": 1}, {"covered": 0, "count": 0}):
            data = report()
            data["data"][0]["totals"]["lines"] = invalid
            self.assertEqual(self.check_report(data).returncode, 2)


if __name__ == "__main__":
    unittest.main()
