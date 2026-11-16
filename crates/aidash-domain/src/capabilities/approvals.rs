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
	/// Allows this approval once and creates an allow Approval Rule for its
	/// kind and target. Only the person the Run's delegation chain starts at
	/// may choose it.
	AllowAlways,
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

/// How an approval that someone else decided first was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecidedOutcome {
	Allowed,
	Denied,
}

/// Whether `subject` is an Eligible approver of what `evaluation` names: an
/// enabled person whom the Tenant's policy lets approve it, and who is its
/// requester or holds `capability_approver`.
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

/// Judge the starter of a Workbench test without a Run or workspace context.
pub fn workbench_approver_eligible(
	bundle: &PolicyBundle,
	starter: &str,
	target: &crate::policy::Resource,
) -> bool {
	let evaluation = Evaluation {
		subject: starter.into(),
		action: "capability.approve".into(),
		resource: target.clone(),
		environment: json!({}),
	};
	let bundle = workbench_policy(bundle);
	approver_eligible(&bundle, &evaluation, starter)
}

/// A Workbench test has no Run or workspace. An allow that depends on either
/// cannot grant access; denies still evaluate their other applicable branches.
pub fn workbench_policy(bundle: &PolicyBundle) -> PolicyBundle {
	use crate::policy::{Condition, Effect, Operand};
	// Do not make an invalid bundle valid by filtering a malformed statement.
	if bundle.validate().is_err() {
		return bundle.clone();
	}
	fn run_operand(operand: &Operand) -> bool {
		match operand {
			Operand::Resource { path } | Operand::Environment { path } => {
				["/run_id", "/workspace_id", "/workspace", "/owner"]
					.iter()
					.any(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")))
			}
			_ => false,
		}
	}
	fn run_condition(condition: &Condition) -> bool {
		match condition {
			Condition::All { conditions } | Condition::Any { conditions } => {
				conditions.iter().any(run_condition)
			}
			Condition::Eq { left, right }
			| Condition::NotEq { left, right }
			| Condition::Contains { left, right } => run_operand(left) || run_operand(right),
			Condition::Exists { value } => run_operand(value),
		}
	}
	let mut scoped = bundle.clone();
	scoped.policies.retain(|policy| {
		policy.effect != Effect::Allow || !policy.condition.as_ref().is_some_and(run_condition)
	});
	scoped
}
/// The state of a new approval: `approvable` says whether any Eligible
/// approver may decide it now.
pub fn requested_state(direct: bool, grant: bool, approvable: bool) -> &'static str {
	if direct || grant {
		"approved"
	} else if approvable {
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
	json!({"operation_id":record.id,"approval_id":record.id,"status":status,"approval_state":if expired&&record.state=="pending"{"expired"}else{&record.state},"approval_revision":record.revision,"targets":record.data["targets"],"decided_by":record.data["decided_by"],"expires_at":record.expires_at,"grant_id":record.data["grant_id"],"effects_may_have_occurred":record.data["attempted_at"].is_string(),"error":record.data["error"]})
}

#[cfg(test)]
mod tests;
