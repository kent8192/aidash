//! Durable approvals permit scoped absence of an allow, never an explicit deny.
use crate::capabilities::records::Record;
use crate::policy::{Evaluation, PolicyBundle, SubjectKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
// Serializable approvals contracts.
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outbound {
	pub idempotency_key: Uuid,
	pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChoice {
	#[default]
	AllowOnce,
	AllowRun,
	Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecision {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	#[serde(default)]
	pub choice: ApprovalChoice,
	pub targets: Option<Vec<String>>,
	pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revoke {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
}

use uuid::Uuid;

pub fn approver_eligible(bundle: &PolicyBundle, evaluation: &Evaluation, requester: &str) -> bool {
	bundle
		.subjects
		.get(&evaluation.subject)
		.is_some_and(|subject| {
			subject.enabled
				&& subject.kind == SubjectKind::User
				&& (evaluation.subject == requester
					|| subject.attributes["capability_approver"] == true)
		}) && bundle.evaluate(evaluation).allowed
}
pub fn requested_state(direct: bool, grant: bool, designated: bool) -> &'static str {
	if direct || grant {
		"approved"
	} else if designated {
		"pending"
	} else {
		"blocked"
	}
}
pub fn projection(record: &Record, expired: bool) -> Value {
	let status = match record.state.as_str() {
		"pending" if !expired => "approval_required",
		"pending" | "denied" | "revoked" | "blocked" => "blocked",
		"approved" | "attempted" => "running",
		other => other,
	};
	json!({"operation_id":record.id,"approval_id":record.id,"status":status,"approval_state":if expired&&record.state=="pending"{"expired"}else{&record.state},"approval_revision":record.revision,"targets":record.data["targets"],"approver":record.data["approver"],"expires_at":record.expires_at,"grant_id":record.data["grant_id"],"effects_may_have_occurred":record.data["attempted_at"].is_string(),"error":record.data["error"]})
}

#[cfg(test)]
mod tests;
