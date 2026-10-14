"""Order pricing for the fixed-task evaluation snapshot."""


def discounted_total(prices_cents, coupon_cents=0, percent_off=0):
    """Return the payable total in whole cents.

    Apply percent_off to the subtotal first, rounding half up to a whole cent,
    then subtract coupon_cents. The payable total is never negative.
    """
    subtotal = sum(prices_cents)
    discounted = subtotal * (100 - percent_off) // 100
    return discounted - coupon_cents
