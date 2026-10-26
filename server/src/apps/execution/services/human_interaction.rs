//! Request replay, Home journal approval expiration and answer validation without I/O.
use crate::apps::execution::serializers::human_requests::HumanRequest;
use crate::{
	Error, Result,
	domain::{Run, nonempty},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

pub(crate) fn validate_request(kind: &str, prompt: &str) -> Result<()> {
	if !matches!(
		kind,
		"QUESTION"
			| "APPROVAL_REQUIRED"
			| "CONFIRMATION"
			| "INFORMATION_REQUEST"
			| "MCP_ELICITATION"
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

/// Whether the human request a Run executing for another Node sent to that
/// Node asking for a Tool call approval has lapsed. Tool call approvals of a
/// Run on this Node are capability approvals and follow `approval_seconds`
/// (ADR 0075).
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
) -> Result<Answer> {
	validate_response(&response)?;
	if let Some(existing) = &request.response {
		if existing == &response {
			return Ok(Answer::Replay);
		}
		return Err(Error::Conflict("human request already answered".into()));
	}
	if matches!(&run.state, crate::domain::RunState::Waiting(s) if matches!(s.as_ref(), crate::domain::WaitingState::Reconciliation { .. }))
		&& response.get("result").is_none()
	{
		return Err(Error::Invalid(
			"tool reconciliation requires a JSON object containing result".into(),
		));
	}
	if request.kind == "MCP_ELICITATION" {
		let prompt: Value = serde_json::from_str(&request.prompt)?;
		let deadline: DateTime<Utc> = serde_json::from_value(prompt["answer_by"].clone())?;
		if actor != "system" && deadline <= Utc::now() {
			return Err(Error::Conflict("MCP_INPUT_STALE_OR_EXPIRED".into()));
		}
		let form: aidash_domain::agent_plugin::interaction::ElicitationRequest =
			serde_json::from_str(&request.prompt)?;
		// Only the system's deadline closure may use the generic unanswered marker.
		if actor != "system" || response.get("answered").is_none() {
			if !matches!(
				response["action"].as_str(),
				Some("accept" | "decline" | "cancel")
			) {
				return Err(Error::Invalid("MCP_ELICITATION_ANSWER_INVALID".into()));
			}
			let value = form.response(&response)?;
			if value["action"] == "accept" {
				aidash_application::tools::validate_form_answer(&form.schema, &value["content"])?;
			}
		}
	}

	Ok(Answer::Write {
		response,
		actor: actor.into(),
	})
}
