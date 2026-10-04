//! Model reservations retain unknown usage and refund only complete bounded reports.
use crate::{Error, Result, provider::ModelResponse};

pub fn reservation_amount(window: usize, output: u32) -> Result<i64> {
	window
		.checked_add(output as usize)
		.and_then(|amount| i64::try_from(amount).ok())
		.ok_or_else(|| Error::Invalid("model reservation overflow".into()))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accounting {
	pub reported: Option<i64>,
	pub refund: i64,
	pub exceeded: bool,
}
pub fn accounting(amount: i64, response: &ModelResponse) -> Accounting {
	let reported = response
		.input_tokens
		.checked_add(response.output_tokens)
		.and_then(|tokens| i64::try_from(tokens).ok());
	let refund = reported
		.filter(|tokens| response.usage_complete && *tokens > 0 && *tokens <= amount)
		.map_or(0, |tokens| amount - tokens);
	Accounting {
		reported,
		refund,
		exceeded: reported.is_some_and(|tokens| tokens > amount),
	}
}

/// Remote settlement accepts complete positive usage independently of its reservation bound.
pub fn complete_reported_usage(response: &ModelResponse) -> Option<i64> {
	response
		.input_tokens
		.checked_add(response.output_tokens)
		.and_then(|tokens| i64::try_from(tokens).ok())
		.filter(|tokens| response.usage_complete && *tokens > 0)
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	#[rstest]
	#[case::complete(8, 2, true, Some(10))]
	#[case::incomplete(8, 2, false, None)]
	#[case::empty(0, 0, true, None)]
	#[case::input_only(8, 0, true, Some(8))]
	#[case::output_only(0, 2, true, Some(2))]
	#[case::maximum(i64::MAX as u64, 0, true, Some(i64::MAX))]
	#[case::database_overflow(i64::MAX as u64, 1, true, None)]
	#[case::unsigned_overflow(u64::MAX, 1, true, None)]
	fn remote_reports_keep_complete_positive_database_sized_usage(
		#[case] input_tokens: u64,
		#[case] output_tokens: u64,
		#[case] usage_complete: bool,
		#[case] expected: Option<i64>,
	) {
		let response = ModelResponse {
			input_tokens,
			output_tokens,
			usage_complete,
			..Default::default()
		};
		assert_eq!(complete_reported_usage(&response), expected);
	}
}
