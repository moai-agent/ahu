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
            "questions": {"route": {"type": "choice", "options": {"billing": "Payments", "technical": "Product"}}},
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
        self.assertEqual(row["score"], 1.0)

    def test_weighted_score_is_normalized_and_invalid_probability_is_rejected(self):
        case = {
            "id": "weighted", "corpus_version": "1.0.0",
            "expected": {"route": "billing", "confidence": {"minimum": 0.8}},
            "questions": {
                "route": {"type": "choice", "options": {"billing": "Payments", "technical": "Product"}},
                "confidence": {"type": "probability"},
            },
            "scoring": {"route": 1.0, "confidence": 1.0, "exact_match_pass_threshold": 1.0},
        }
        row = local_eval.score(case, {"route": "billing", "confidence": 0.4}, {}, None, None, "d")
        self.assertEqual(row["score"], 0.5)
        self.assertFalse(row["passed"])
        with self.assertRaises(ValueError):
            local_eval.score(case, {"route": "billing", "confidence": 100}, {}, None, None, "d")


if __name__ == "__main__":
    unittest.main()
