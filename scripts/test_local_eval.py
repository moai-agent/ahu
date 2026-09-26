import importlib.util
import unittest
from pathlib import Path


SPEC = importlib.util.spec_from_file_location(
    "local_eval", Path(__file__).with_name("local_eval.py")
)
local_eval = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(local_eval)


class LocalEvalRecordTests(unittest.TestCase):
    def test_manual_counts_do_not_claim_otel_observation(self):
        case = {
            "id": "synthetic-case",
            "corpus_version": "1.0.0",
            "expected": {"route": "billing"},
            "scoring": {"route": 1.0, "exact_match_pass_threshold": 1.0},
        }
        row = local_eval.score(
            case,
            {"route": "billing"},
            {},
            decision_calls=2,
            elapsed_ms=None,
            case_digest="synthetic-digest",
        )
        self.assertEqual(row["decision_call_count"], 2)
        self.assertEqual(row["telemetry_coverage"], "none")
        self.assertFalse(row["mcp_observed"])


if __name__ == "__main__":
    unittest.main()
