use super::{settle_while_claimed, settled};
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

fn pool_timeout() -> Error {
	Error::Database(sqlx::Error::PoolTimedOut)
}

/// #728: an outage that ends before the claim expires leaves the claim's attempt to be restored.
#[rstest]
#[tokio::test(start_paused = true)]
async fn settlement_waits_out_an_outage_instead_of_leaving_the_claim_attempt_spent() {
	// Arrange
	let writes = std::cell::Cell::new(0);
	let writes = &writes;
	// Act
	let result = settle_while_claimed(|| async move {
		writes.set(writes.get() + 1);
		if writes.get() < 4 {
			Err(pool_timeout())
		} else {
			Ok(())
		}
	})
	.await;
	// Assert
	result.unwrap();
	assert_eq!(writes.get(), 4);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn settlement_does_not_repeat_a_failure_that_is_not_the_database() {
	// Arrange
	let writes = std::cell::Cell::new(0);
	let writes = &writes;
	// Act
	let result = settle_while_claimed(|| async move {
		writes.set(writes.get() + 1);
		Err(Error::Conflict("engine claim was replaced".into()))
	})
	.await;
	// Assert
	assert!(matches!(result, Err(Error::Conflict(_))));
	assert_eq!(writes.get(), 1);
}

/// An outage past expiry does not stop settlement when the database recovers.
#[rstest]
#[tokio::test(start_paused = true)]
async fn settlement_succeeds_after_the_claim_expires() {
	// Arrange
	let started = tokio::time::Instant::now();
	let expires = started + std::time::Duration::from_secs(30);
	// Act
	let result = settle_while_claimed(|| async {
		if tokio::time::Instant::now() < expires {
			Err(pool_timeout())
		} else {
			Ok(())
		}
	})
	.await;
	// Assert
	result.unwrap();
	let waited = started.elapsed();
	assert!(waited >= std::time::Duration::from_secs(30), "{waited:?}");
	assert!(waited < std::time::Duration::from_secs(36), "{waited:?}");
}
