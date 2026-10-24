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
/// Local settlement reports only complete positive usage; unknown usage keeps the
/// full reservation and never counts as exceeding it. Cache read and write counts
/// are breakdowns of the prompt and are never added again.
pub fn accounting(amount: i64, response: &ModelResponse) -> Accounting {
	let reported = complete_reported_usage(response);
	let refund = reported
		.filter(|tokens| *tokens <= amount)
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

	#[rstest]
	#[case::complete(30, 10, true, Some(40), 90, false)]
	#[case::incomplete_is_unknown(30, 10, false, None, 0, false)]
	#[case::incomplete_never_exceeds(100, 31, false, None, 0, false)]
	#[case::empty(0, 0, true, None, 0, false)]
	#[case::exact(100, 30, true, Some(130), 0, false)]
	#[case::exceeded(100, 31, true, Some(131), 0, true)]
	#[case::database_overflow(i64::MAX as u64, 1, true, None, 0, false)]
	fn local_accounting_reports_only_complete_positive_usage(
		#[case] input_tokens: u64,
		#[case] output_tokens: u64,
		#[case] usage_complete: bool,
		#[case] reported: Option<i64>,
		#[case] refund: i64,
		#[case] exceeded: bool,
	) {
		let response = ModelResponse {
			input_tokens,
			output_tokens,
			usage_complete,
			..Default::default()
		};
		assert_eq!(
			accounting(130, &response),
			Accounting {
				reported,
				refund,
				exceeded
			}
		);
	}
}
