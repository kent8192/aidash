import unittest

from pricing import discounted_total


class VisiblePricingTest(unittest.TestCase):
    def test_percent_discount_rounds_half_up(self):
        self.assertEqual(discounted_total([333], percent_off=50), 167)

    def test_coupon_after_percent_discount(self):
        self.assertEqual(discounted_total([1000, 1000], coupon_cents=300, percent_off=10), 1500)


if __name__ == "__main__":
    unittest.main()
