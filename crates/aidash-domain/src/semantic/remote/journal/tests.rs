use super::*;
use chrono::Duration;
fn record(state: &str) -> Record {
	Record {
		id: Uuid::new_v4(),
		home_node: "home".into(),
		grant_id: Uuid::new_v4(),
		admission_id: Uuid::new_v4(),
		digest: "digest".into(),
		binding: Value::Null,
		state: state.into(),
		cycle: 2,
		failures: 0,
		attempt_id: Some(Uuid::new_v4()),
		fence: 7,
		lease_until: None,
		next_attempt: None,
		error: None,
		receipt: None,
	}
}
#[rstest::rstest]
fn retry_schedule_does_not_reset_persisted_failures() {
	assert_eq!(
		(0..=6).map(retry_delay).collect::<Vec<_>>(),
		vec![None, Some(2), Some(4), Some(8), Some(16), Some(32), None]
	);
	assert_eq!(retry_delay(i32::MAX), None);
}
#[rstest::rstest]
fn dispatched_lease_expiry_creates_backoff_instead_of_a_new_attempt() {
	let now = Utc::now();
	let mut r = record("ACTIVE");
	r.lease_until = Some(now - Duration::seconds(1));
	assert!(matches!(
		claim_plan(&r, now),
		ClaimPlan::Expired {
			failures: 1,
			delay: Some(2)
		}
	));
	r.failures = 5;
	assert!(matches!(
		claim_plan(&r, now),
		ClaimPlan::Expired {
			failures: 6,
			delay: None
		}
	));
}
#[rstest::rstest]
#[case("PENDING")]
#[case("ACTIVE")]
fn an_unexpired_fence_is_not_claimable(#[case] state: &str) {
	let now = Utc::now();
	let mut r = record(state);
	r.lease_until = Some(now + Duration::seconds(1));
	assert!(matches!(
		claim_plan(&r, now),
		ClaimPlan::Rejected(Failure::Pending)
	));
}
#[rstest::rstest]
fn an_unreached_retry_due_time_is_not_claimable() {
	let now = Utc::now();
	let mut r = record("WAITING");
	r.next_attempt = Some(now + Duration::seconds(1));
	assert!(matches!(
		claim_plan(&r, now),
		ClaimPlan::Rejected(Failure::Pending)
	));
}
#[rstest::rstest]
#[case("INVALIDATED", Failure::Invalidated)]
#[case("CANCELLED", Failure::Authority)]
#[case("PAUSED", Failure::Unavailable)]
fn terminal_authority_or_failure_blocks_claims(#[case] state: &str, #[case] reason: Failure) {
	assert!(
		matches!(claim_plan(&record(state),Utc::now()),ClaimPlan::Rejected(failure) if failure==reason)
	);
}
#[rstest::rstest]
fn ready_receipts_are_reused_without_reserving_an_attempt() {
	assert!(matches!(
		claim_plan(&record("READY"), Utc::now()),
		ClaimPlan::Ready
	));
}
#[rstest::rstest]
fn a_pending_operation_can_start_its_first_attempt() {
	assert!(matches!(
		claim_plan(&record("PENDING"), Utc::now()),
		ClaimPlan::Fresh
	));
}
#[rstest::rstest]
fn transient_exhaustion_is_durable_and_permanent_failures_do_not_consume_retries() {
	let r = retry(5, Failure::Unavailable);
	assert_eq!(r.failure, Failure::RetriesExhausted);
	assert_eq!(r.state, "PAUSED");
	assert_eq!(r.failures, 6);
	assert!(r.delay.is_none());
	let r = retry(3, Failure::Invalidated);
	assert_eq!(r.failure, Failure::Invalidated);
	assert_eq!(r.state, "INVALIDATED");
	assert_eq!(r.failures, 3);
	assert!(r.delay.is_none());
}
