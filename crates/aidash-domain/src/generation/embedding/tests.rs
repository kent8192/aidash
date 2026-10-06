use super::*;
use rstest::rstest;

#[rstest]
#[case(0, 1024)]
#[case(6, 1030)]
fn input_bytes_include_the_existing_allowance(#[case] bytes: usize, #[case] expected: i64) {
	assert_eq!(reservation_amount(bytes).unwrap(), expected);
}

#[rstest]
#[case(usize::MAX)]
#[case(i64::MAX as usize)]
fn unrepresentable_reservations_are_rejected(#[case] bytes: usize) {
	assert!(
		matches!(reservation_amount(bytes), Err(Error::Invalid(message)) if message == "embedding input reservation overflow")
	);
}

#[rstest]
fn largest_representable_reservation_does_not_wrap() {
	let bytes = (i64::MAX as usize).saturating_sub(1024);
	assert_eq!(reservation_amount(bytes).unwrap(), bytes as i64 + 1024);
}

#[rstest]
#[case(None, None, 0, false)]
#[case(Some(0), Some(0), 0, false)]
#[case(Some(10), Some(10), 1020, false)]
#[case(Some(1030), Some(1030), 0, false)]
#[case(Some(1031), Some(1031), 0, true)]
#[case(Some(u64::MAX), None, 0, true)]
fn only_positive_bounded_usage_refunds_the_reservation(
	#[case] reported: Option<u64>,
	#[case] stored: Option<i64>,
	#[case] refund: i64,
	#[case] exceeded: bool,
) {
	assert_eq!(
		accounting(1030, reported),
		Accounting {
			reported: stored,
			refund,
			exceeded
		}
	);
}
