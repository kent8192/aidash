//! Database rows are converted explicitly into portable source execution state.
use aidash_domain::federation::execution::home;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone)]
pub(crate) struct Grant {
	pub(crate) node: String,
	pub(crate) id: Uuid,
	pub(crate) task_id: Uuid,
	pub(crate) task_revision: i64,
	pub(crate) thread_id: Uuid,
	pub(crate) node_id: String,
	pub(crate) tenant: String,
	pub(crate) credential_id: Uuid,
	pub(crate) root_subject: String,
	pub(crate) subject_chain: Vec<String>,
	pub(crate) inspection: Value,
	pub(crate) expires_at: DateTime<Utc>,
	pub(crate) revoked: bool,
	pub(crate) semantic: Value,
}
crate::native_record!(Grant {
	node,
	id,
	task_id,
	task_revision,
	thread_id,
	node_id,
	tenant,
	credential_id,
	root_subject,
	subject_chain,
	inspection,
	expires_at,
	revoked,
	semantic
});

#[derive(Clone)]
pub(crate) struct HomeBinding {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub task_id: Uuid,
	pub task_revision: i64,
	pub initial_task: Value,
	pub activated_at: Option<DateTime<Utc>>,
}
crate::native_record!(HomeBinding {
	grant_id,
	admission_id,
	task_id,
	task_revision,
	initial_task,
	activated_at
});

impl From<Grant> for home::Grant {
	fn from(row: Grant) -> Self {
		Self {
			id: row.id,
			task_id: row.task_id,
			task_revision: row.task_revision,
			thread_id: row.thread_id,
			node_id: row.node_id,
			tenant: row.tenant,
			credential_id: row.credential_id,
			root_subject: row.root_subject,
			subject_chain: row.subject_chain,
			inspection: row.inspection,
			expires_at: row.expires_at,
			revoked: row.revoked,
			semantic: row.semantic,
		}
	}
}

impl From<HomeBinding> for home::HomeBinding {
	fn from(row: HomeBinding) -> Self {
		Self {
			grant_id: row.grant_id,
			admission_id: row.admission_id,
			task_id: row.task_id,
			task_revision: row.task_revision,
			initial_task: row.initial_task,
			activated: row.activated_at.is_some(),
		}
	}
}

/// The SQL condition that the `authorization_remote_grants` row named by
/// `grant` (its table name or alias) is live: the rule of
/// [`home::Liveness`] (#1013). Its lifetime bounds it until this grant's own
/// Home binding has recorded the receiver's activation and the receiver holds
/// the Task (its owner is the last subject of the grant's chain, which only
/// that receiver's claim makes it); from then on it lives while the Task has
/// not ended, however long the Run's Stint sleeps. The Task's status never
/// stands in for the activation. Every Home check of a grant's liveness uses
/// this condition, so none of them reads `expires_at` alone.
pub(crate) fn live_condition(grant: &str) -> String {
	let ended = home::ending_statuses()
		.map(|status| format!("'{}'", status.as_str()))
		.collect::<Vec<_>>()
		.join(", ");
	format!(
		"(NOT {grant}.revoked AND ({grant}.expires_at > CLOCK_TIMESTAMP() OR EXISTS (SELECT 1 FROM authorization_remote_execution live_binding JOIN tasks live_task ON live_task.id = live_binding.task_id AND live_task.tenant = live_binding.tenant WHERE live_binding.grant_id = {grant}.id AND live_binding.task_id = {grant}.task_id AND live_binding.activated_at IS NOT NULL AND live_task.owner = {grant}.subject_chain[array_length({grant}.subject_chain, 1)] AND live_task.status NOT IN ({ended}))))"
	)
}

#[cfg(test)]
mod live_condition_tests {
	#[test]
	fn the_condition_needs_this_grants_activation_its_executor_and_an_open_task() {
		let condition = super::live_condition("g");
		assert!(condition.starts_with("(NOT g.revoked AND (g.expires_at > CLOCK_TIMESTAMP()"));
		assert!(
			condition.contains("live_binding.grant_id = g.id AND live_binding.task_id = g.task_id")
		);
		assert!(condition.contains("live_binding.activated_at IS NOT NULL"));
		assert!(
			condition
				.contains("live_task.owner = g.subject_chain[array_length(g.subject_chain, 1)]")
		);
		assert!(condition.contains("NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED')"));
		// The Task's status never stands in for the activation.
		assert!(!condition.contains("<> 'OPEN'"));
		assert_eq!(
			condition.matches('(').count(),
			condition.matches(')').count()
		);
	}
}

pub(crate) mod authority;

pub(crate) mod persistence;

pub(crate) mod semantic;

pub(crate) mod reads;

pub(crate) mod provenance;

pub(crate) mod search;

pub(crate) mod snapshot;
