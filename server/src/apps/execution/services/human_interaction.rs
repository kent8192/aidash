//! Request replay, approval expiration and answer validation without I/O.
use crate::apps::execution::serializers::human_requests::HumanRequest;
use crate::{
	Error, Result,
	domain::{Run, nonempty},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

pub(crate) fn validate_request(kind: &str, prompt: &str) -> Result<()> {
	if !matches!(
		kind,
		"QUESTION" | "APPROVAL_REQUIRED" | "CONFIRMATION" | "INFORMATION_REQUEST"
	) {
		return Err(Error::Invalid("unknown human request kind".into()));
	}
	nonempty(prompt, "human request prompt")
}

pub(crate) fn validate_replay(
	request: &HumanRequest,
	run: &Run,
	kind: &str,
	prompt: &str,
) -> Result<()> {
	if request.run_id != run.id || request.kind != kind || request.prompt != prompt {
		return Err(Error::Conflict(
			"human request key reused with different input".into(),
		));
	}
	Ok(())
}

pub(crate) fn approval_expired(request: &HumanRequest, now: DateTime<Utc>) -> bool {
	request.kind == "APPROVAL_REQUIRED" && request.created_at + Duration::minutes(15) <= now
}

pub(crate) enum Answer {
	Replay,
	Write { response: Value, actor: String },
}

pub(crate) fn validate_response(response: &Value) -> Result<()> {
	if response.is_null() {
		return Err(Error::Invalid("a human response cannot be null".into()));
	}
	Ok(())
}

pub(crate) fn answer(
	request: &HumanRequest,
	run: &Run,
	response: Value,
	actor: &str,
	now: DateTime<Utc>,
) -> Result<Answer> {
	validate_response(&response)?;
	let expired = approval_expired(request, now)
		&& matches!(&run.state, crate::domain::RunState::Waiting(s) if matches!(s.as_ref(), crate::domain::WaitingState::ExternalApproval { request_id, .. } if *request_id == request.id));
	let automatic_expiry = expired && (actor == "system" || request.response.is_none());
	let (response, actor) = if automatic_expiry {
		(json!({"approved": false, "expired": true}), "system")
	} else {
		(response, actor)
	};
	if let Some(existing) = &request.response {
		if existing == &response {
			return Ok(Answer::Replay);
		}
		if !automatic_expiry {
			return Err(Error::Conflict("human request already answered".into()));
		}
	}
	if matches!(&run.state, crate::domain::RunState::Waiting(s) if matches!(s.as_ref(), crate::domain::WaitingState::Reconciliation { .. }))
		&& response.get("result").is_none()
	{
		return Err(Error::Invalid(
			"tool reconciliation requires a JSON object containing result".into(),
		));
	}
	Ok(Answer::Write {
		response,
		actor: actor.into(),
	})
}
