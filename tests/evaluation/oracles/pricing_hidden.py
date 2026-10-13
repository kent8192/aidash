"""Held-out regression checks for the pricing snapshot.

The evaluator never sends this file to an Agent. It runs next to the Agent's
proposed pricing.py, so a change that only satisfies the visible tests fails here.
"""
import unittest

from pricing import discounted_total


class HiddenPricingRegression(unittest.TestCase):
    def test_coupon_larger_than_total_is_free_not_negative(self):
        self.assertEqual(discounted_total([500], coupon_cents=800), 0)

    def test_empty_order(self):
        self.assertEqual(discounted_total([]), 0)

    def test_half_cent_after_discount_rounds_up(self):
        self.assertEqual(discounted_total([1999, 1], percent_off=15), 1700)
        self.assertEqual(discounted_total([105], percent_off=10), 95)


if __name__ == "__main__":
    unittest.main()
