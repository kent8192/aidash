"""Independent numeric and decision contracts for the frozen projection gates.

The tests own numerical interval boundaries and public malformed-evidence
behavior. They do not assert the implementation's integration or call shape.
"""
import copy
import json
import math
import unittest

import projection_statistics as stats


def strata(repetitions=10, baseline=True, candidate=True,
           baseline_cost=1.0, candidate_cost=0.8):
    return [
        {"task": task, "weight": weight, "independent_blocks": True,
         "cost_bounds_enforced": True, "baseline_cost_bound": 1.0,
         "candidate_cost_bound": 0.8,
         "pairs": [{"baseline_success": baseline, "candidate_success": candidate,
                    "baseline_cost": baseline_cost, "candidate_cost": candidate_cost}
                   for _ in range(repetitions)]}
        for task, weight in stats.PRIMARY_WEIGHTS.items()
    ]


class ExactBinomialTest(unittest.TestCase):
    def test_published_central_binomial_boundary(self):
        # The familiar two-sided 95% exact interval for 5/10 is also the pair
        # of one-sided 97.5% limits. These values are independent beta quantiles.
        interval = stats.clopper_pearson(5, 10, 0.025)
        self.assertAlmostEqual(interval[0], 0.18708602844739855, places=11)
        self.assertAlmostEqual(interval[1], 0.8129139715526015, places=11)

    def test_extreme_counts_have_analytic_nonzero_width(self):
        self.assertEqual(stats.clopper_pearson(0, 10, 0.025)[0], 0.0)
        self.assertAlmostEqual(stats.clopper_pearson(0, 10, 0.025)[1],
                               1 - 0.025 ** 0.1, places=13)
        self.assertAlmostEqual(stats.clopper_pearson(10, 10, 0.025)[0],
                               0.025 ** 0.1, places=13)
        self.assertEqual(stats.clopper_pearson(10, 10, 0.025)[1], 1.0)

    def test_interior_limits_invert_the_independent_binomial_polynomial(self):
        for count, total in [(1, 3), (2, 7), (4, 11), (9, 11)]:
            lower, upper = stats.clopper_pearson(count, total, .0025)
            lower_tail = sum(math.comb(total, k) * lower ** k * (1 - lower) ** (total - k)
                             for k in range(count, total + 1))
            upper_tail = sum(math.comb(total, k) * upper ** k * (1 - upper) ** (total - k)
                             for k in range(count + 1))
            self.assertAlmostEqual(lower_tail, .0025, places=12)
            self.assertAlmostEqual(upper_tail, .0025, places=12)

    def test_invalid_counts_and_error_probability_are_refused(self):
        for count, total, delta in [(True, 10, .025), (1, False, .025),
                                    (1.0, 10, .025), (-1, 10, .025),
                                    (11, 10, .025), (0, 0, .025),
                                    (1, 10, math.nan), (1, 10, True)]:
            with self.subTest(count=count, total=total, delta=delta):
                with self.assertRaises(ValueError):
                    stats.clopper_pearson(count, total, delta)


class QualityGateTest(unittest.TestCase):
    def test_ten_ties_are_not_noninferiority_and_117_is_first_passing_size(self):
        # With five zero-discordance strata the lower limit is exactly
        # -(1-.0025**(1/n)); the strict -.05 crossing is 117, not 116.
        for repetitions, status in [(10, "inconclusive"), (116, "inconclusive"),
                                    (117, "pass")]:
            with self.subTest(repetitions=repetitions):
                result = stats.quality_bound(strata(repetitions))
                self.assertEqual(result["status"], status)
                self.assertAlmostEqual(result["bounds"]["lower"],
                                       -(1 - .0025 ** (1 / repetitions)), places=12)

    def test_clear_paired_regression_fails_even_with_successful_baseline(self):
        result = stats.quality_bound(strata(100, candidate=False))
        self.assertEqual(result["status"], "fail")
        self.assertLess(result["bounds"]["upper"], -.05)
        denominator = result["denominators"]["structured-data-overdue-totals"]
        self.assertEqual(denominator, {"pairs": 100, "baseline_successes": 100,
                                       "candidate_successes": 0})

    def test_duplicate_coding_ids_share_one_quarter_of_population(self):
        evidence = strata(100)
        for entry in evidence:
            coding = entry["task"].startswith("code-change")
            for pair in entry["pairs"]:
                pair["baseline_success"] = coding
                pair["candidate_success"] = not coding
        result = stats.quality_bound(evidence)
        self.assertAlmostEqual(result["estimates"]["success_difference"], .5)
        self.assertAlmostEqual(result["bounds"]["lower"],
                               .75 * (2 * .0025 ** .01 - 1) - .25, places=12)

    def test_fixed_population_weights_and_independence_cannot_be_silently_changed(self):
        cases = []
        invalid = strata(250)
        invalid[0]["weight"], invalid[2]["weight"] = .25, .125
        cases.append(invalid)
        cases.append(strata(250)[:-1])
        invalid = strata(250)
        invalid[1]["task"] = invalid[0]["task"]
        cases.append(invalid)
        invalid = strata(250)
        del invalid[0]["independent_blocks"]
        cases.append(invalid)
        for invalid in cases:
            with self.subTest(invalid=invalid[0].get("task")):
                result = stats.quality_bound(invalid)
                self.assertEqual(result["status"], "inconclusive")
                self.assertTrue(result["reasons"])

    def test_numeric_flags_and_nan_weights_are_not_valid_success_evidence(self):
        for field, value in [("baseline_success", 1), ("candidate_success", None)]:
            invalid = strata(250)
            invalid[0]["pairs"][0][field] = value
            self.assertEqual(stats.quality_bound(invalid)["status"], "inconclusive")
        invalid = strata(250)
        invalid[0]["weight"] = math.nan
        self.assertEqual(stats.quality_bound(invalid)["status"], "inconclusive")
        invalid[0]["task"] = []
        self.assertEqual(stats.quality_bound(invalid)["status"], "inconclusive")


class CostGateTest(unittest.TestCase):
    def test_bounded_constant_cost_still_has_positive_radius(self):
        result = stats.cost_ratio_bound(strata(250))
        self.assertEqual(result["status"], "pass")
        self.assertGreater(result["bounds"]["upper"], .8)
        self.assertLess(result["bounds"]["upper"], .9)
        self.assertLess(result["bounds"]["baseline_mean_cost_lower"], 1)

    def test_small_sample_zero_denominator_is_json_null_with_reason(self):
        result = stats.cost_ratio_bound(strata(10))
        self.assertEqual(result["status"], "inconclusive")
        self.assertIsNone(result["bounds"]["upper"])
        self.assertTrue(any("denominator" in reason for reason in result["reasons"]))
        json.dumps(result, allow_nan=False)

    def test_empirical_variance_contributes_to_the_cost_radius(self):
        evidence = strata(1000, candidate_cost=.2)
        for entry in evidence:
            for index, pair in enumerate(entry["pairs"]):
                pair["baseline_cost"] = float(index % 2)
        result = stats.cost_ratio_bound(evidence)
        # Exactly 500 zeros and 500 ones: mean=.5, unbiased variance=250/999.
        log_term = math.log(1600)
        radius = math.sqrt(2 * (250 / 999) * log_term / 1000) + 7 * log_term / (3 * 999)
        self.assertAlmostEqual(result["bounds"]["baseline_mean_cost_lower"], .5 - radius, places=12)

    def test_unknown_unbounded_unenforced_or_nonfinite_cost_never_passes(self):
        for value in [None, "unknown", math.nan, math.inf, True, -1]:
            invalid = strata(250)
            invalid[0]["pairs"][0]["candidate_cost"] = value
            with self.subTest(value=value):
                result = stats.cost_ratio_bound(invalid)
                self.assertEqual(result["status"], "inconclusive")
                json.dumps(result, allow_nan=False)
        for key, value in [("baseline_cost_bound", None),
                           ("candidate_cost_bound", 0.5),
                           ("cost_bounds_enforced", False)]:
            invalid = strata(250)
            invalid[0][key] = value
            self.assertEqual(stats.cost_ratio_bound(invalid)["status"], "inconclusive")

    def test_failed_work_cost_is_included_in_the_cost_per_success(self):
        # One half of candidate attempts fail but still incur twice the
        # baseline cost. Omitting their cost would incorrectly hide regression.
        evidence = strata(1000, baseline_cost=.2, candidate_cost=.4)
        for entry in evidence:
            for pair in entry["pairs"][::2]:
                pair["candidate_success"] = False
        result = stats.cost_ratio_bound(evidence)
        self.assertEqual(result["status"], "fail")
        self.assertAlmostEqual(result["estimates"]["candidate_cost_per_success"], .8)
        self.assertAlmostEqual(result["estimates"]["baseline_cost_per_success"], .2)

    def test_no_candidate_successes_has_no_finite_cost_ratio(self):
        result = stats.cost_ratio_bound(strata(250, candidate=False))
        self.assertEqual(result["status"], "inconclusive")
        self.assertIsNone(result["bounds"]["upper"])
        self.assertIsNone(result["estimates"]["candidate_cost_per_success"])


class PlanningTest(unittest.TestCase):
    def test_ten_successful_ties_do_not_manufacture_a_power_distribution(self):
        pilot = {"routes": [{"route": route, "strata": strata()}
                            for route in ("automatic", "explicit")]}
        result = stats.choose_confirmation_n(pilot)
        self.assertEqual(result["status"], "inconclusive")
        self.assertIsNone(result["selected_n"])
        self.assertTrue(any("planning distribution" in reason for reason in result["reasons"]))
        json.dumps(result, allow_nan=False)

    def test_pilot_requires_both_routes_and_frozen_simulation_count(self):
        pilot = {"routes": [{"route": "automatic", "strata": strata()}]}
        self.assertEqual(stats.choose_confirmation_n(pilot)["status"], "inconclusive")
        pilot["routes"].append({"route": "explicit", "strata": strata()})
        result = stats.choose_confirmation_n(pilot, simulations=True)
        self.assertEqual(result["status"], "inconclusive")
        self.assertTrue(any("10000" in reason for reason in result["reasons"]))

    def test_full_frozen_simulation_preserves_tie_uncertainty_and_pilot_covariance(self):
        # A balanced pilot can support a planning distribution; ten successes
        # alone cannot. Under its empirical all-ties model, *every* simulation
        # must cross the known quality boundary between n100 and n125. The
        # sensitivity must include unseen discordance and cannot claim power.
        evidence = strata()
        for entry in evidence:
            for pair in entry["pairs"][:5]:
                pair["baseline_success"] = pair["candidate_success"] = False
        pilot = {"routes": [{"route": route, "strata": copy.deepcopy(evidence)}
                            for route in ("automatic", "explicit")]}
        original = copy.deepcopy(pilot)
        result = stats.choose_confirmation_n(pilot)
        self.assertEqual(pilot, original)
        self.assertEqual(result["status"], "inconclusive")
        self.assertEqual(len(result["power_by_n"]), 9)
        self.assertIsNone(result["selected_n"])
        by_n = {row["n"]: row for row in result["power_by_n"]}
        for route in ("automatic", "explicit"):
            self.assertEqual(by_n[100]["tests"][route]["empirical"]["quality"]["passes"], 0)
            self.assertEqual(by_n[125]["tests"][route]["empirical"]["quality"]["passes"], 10000)
            self.assertLess(by_n[250]["tests"][route]["discordance_sensitivity"]["quality"]["power_lower"], .95)
            covariance = result["pilot_covariance"][route]["structured-data-overdue-totals"]
            self.assertAlmostEqual(covariance["sample_covariance"][0][1], 2.5 / 9)
        json.dumps(result, allow_nan=False)


if __name__ == "__main__":
    unittest.main()
