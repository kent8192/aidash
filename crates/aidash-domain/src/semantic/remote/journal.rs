//! Durable semantic retry planning never treats an expired dispatch lease as an unsent request.
use super::super::Failure;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone)]
pub struct Record {
	pub id: Uuid,
	pub home_node: String,
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub digest: String,
	pub binding: Value,
	pub state: String,
	pub cycle: i32,
	pub failures: i32,
	pub attempt_id: Option<Uuid>,
	pub fence: i64,
	pub lease_until: Option<DateTime<Utc>>,
	pub next_attempt: Option<DateTime<Utc>>,
	pub error: Option<String>,
	pub receipt: Option<Value>,
}
#[derive(Debug, Clone)]
pub struct Attempt {
	pub operation_id: Uuid,
	pub id: Uuid,
	pub fence: i64,
}
pub enum Claim {
	Attempt(Attempt),
	Ready(Box<super::Receipt>),
}
pub fn reason(value: Option<&str>) -> Failure {
	value
		.and_then(|s| serde_json::from_value(json!(s)).ok())
		.unwrap_or(Failure::Unavailable)
}
/// Five automatic retries follow the initial attempt. This counter is stored
/// on the operation, not in an in-memory worker or an HTTP transport loop.
pub fn retry_delay(failures: i32) -> Option<i64> {
	(1..=5).contains(&failures).then(|| 1_i64 << failures)
}
pub enum ClaimPlan {
	Ready,
	Rejected(Failure),
	Expired { failures: i32, delay: Option<i64> },
	Fresh,
}
pub fn claim_plan(record: &Record, now: DateTime<Utc>) -> ClaimPlan {
	match record.state.as_str() {
		"READY" => return ClaimPlan::Ready,
		"PAUSED" => return ClaimPlan::Rejected(reason(record.error.as_deref())),
		"INVALIDATED" => return ClaimPlan::Rejected(Failure::Invalidated),
		"CANCELLED" => return ClaimPlan::Rejected(Failure::Authority),
		_ => {}
	}
	if record.next_attempt.is_some_and(|due| due > now)
		|| record.lease_until.is_some_and(|until| until > now)
	{
		return ClaimPlan::Rejected(Failure::Pending);
	}
	if record.state == "ACTIVE" {
		let failures = record.failures + 1;
		ClaimPlan::Expired {
			failures,
			delay: retry_delay(failures),
		}
	} else {
		ClaimPlan::Fresh
	}
}
pub struct Retry {
	pub failures: i32,
	pub delay: Option<i64>,
	pub failure: Failure,
	pub state: &'static str,
}
pub fn retry(failures: i32, failure: Failure) -> Retry {
	let failures = failures + i32::from(failure.transient());
	let delay = failure.transient().then(|| retry_delay(failures)).flatten();
	let failure = if failure.transient() && delay.is_none() {
		Failure::RetriesExhausted
	} else {
		failure
	};
	let state = if failure == Failure::Invalidated {
		"INVALIDATED"
	} else if delay.is_some() {
		"WAITING"
	} else {
		"PAUSED"
	};
	Retry {
		failures,
		delay,
		failure,
		state,
	}
}
#[cfg(test)]
mod tests;
