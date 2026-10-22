use super::settled;
use crate::{Error, semantic::remote::Failure};
use rstest::rstest;

#[rstest]
#[case::outage(Err(Error::RemoteSemantic(Failure::Unavailable)), false, ("pending", Some("engine_unavailable"), true))]
#[case::final_outage(Err(Error::RemoteSemantic(Failure::Unavailable)), true, ("failed", Some("engine_unavailable"), true))]
#[case::live_model_attempt(Err(Error::RemoteSemantic(Failure::Pending)), true, ("pending", Some("model_call_pending"), false))]
#[case::spent_ledger(Err(Error::RemoteSemantic(Failure::RetriesExhausted)), false, ("failed", Some("invalid_or_exhausted_work"), true))]
#[case::durable_budget(Err(Error::RemoteSemantic(Failure::DurableMemoryBudget)), false, ("failed", Some("invalid_or_exhausted_work"), true))]
#[case::queue_full(Err(Error::Conflict(crate::semantic::repositories::candidates::QUEUE_FULL.into())), true, ("pending", Some("candidate_queue_full"), false))]
#[case::complete(Ok(()), false, ("complete", None, true))]
fn a_live_model_attempt_waits_without_consuming_a_job_attempt(
	#[case] outcome: crate::Result<()>,
	#[case] last: bool,
	#[case] expected: (&str, Option<&str>, bool),
) {
	// Act
	let actual = settled(&outcome, last);
	// Assert
	assert_eq!(actual, expected);
}
