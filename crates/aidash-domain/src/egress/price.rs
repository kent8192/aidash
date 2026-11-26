//! Deployment prices in USD nanocredits per GiB, with cumulative rounding.

#[derive(Clone, Copy, Debug)]
pub struct EgressPrices {
	pub sent: u32,
	pub received: u32,
}

impl Default for EgressPrices {
	fn default() -> Self {
		Self {
			sent: 200_000_000,
			received: 60_000_000,
		}
	}
}

/// The cost of the increase between two cumulative byte counts. Rounding
/// the cumulative values keeps the cost independent of debit batch size.
pub fn cost_increase(price: u32, previous: i64, current: i64) -> i64 {
	let cost = |bytes| i128::from(bytes) * i128::from(price) / (1_i128 << 30);
	i64::try_from(cost(current) - cost(previous)).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_same_transfer_costs_the_same_across_batch_boundaries() {
		let prices = EgressPrices::default();
		let gib = 1 << 30;
		assert_eq!(cost_increase(prices.sent, 0, gib), 200_000_000);
		assert_eq!(cost_increase(prices.received, 0, gib), 60_000_000);
		assert_eq!(
			cost_increase(prices.sent, 0, 11) + cost_increase(prices.sent, 11, gib),
			200_000_000
		);
	}
}
