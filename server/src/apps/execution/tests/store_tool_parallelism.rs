//! The Node Tool Parallelism ceiling must fit beside the worker slots in the pool.
use super::tool_parallelism;
use crate::Error;

#[rstest::rstest]
#[case::default(None, 1, 1)]
#[case::sequential_needs_no_headroom(Some("1"), 1, 1)]
#[case::two(Some("2"), 6, 2)]
#[case::four(Some("4"), 8, 4)]
#[case::large_pool(Some("3"), 64, 3)]
fn tool_parallelism_accepts_bounded_ceilings(
	#[case] value: Option<&str>,
	#[case] pool_connections: usize,
	#[case] expected: usize,
) {
	assert_eq!(tool_parallelism(value, pool_connections).unwrap(), expected);
}

#[rstest::rstest]
#[case::zero(Some("0"), 64)]
#[case::above_maximum(Some("5"), 64)]
#[case::negative(Some("-1"), 64)]
#[case::fraction(Some("2.0"), 64)]
#[case::garbage(Some("many"), 64)]
#[case::empty(Some(""), 64)]
#[case::two_without_headroom(Some("2"), 5)]
#[case::four_without_headroom(Some("4"), 7)]
fn tool_parallelism_rejects_invalid_or_unbacked_ceilings(
	#[case] value: Option<&str>,
	#[case] pool_connections: usize,
) {
	assert!(matches!(
		tool_parallelism(value, pool_connections),
		Err(Error::Invalid(_))
	));
}
