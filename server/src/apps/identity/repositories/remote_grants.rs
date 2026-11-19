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
	pub(crate) workspace_id: Uuid,
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
	workspace_id,
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
}
crate::native_record!(HomeBinding {
	grant_id,
	admission_id,
	task_id,
	task_revision,
	initial_task
});

impl From<Grant> for home::Grant {
	fn from(row: Grant) -> Self {
		Self {
			id: row.id,
			task_id: row.task_id,
			task_revision: row.task_revision,
			workspace_id: row.workspace_id,
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
		}
	}
}

/// The SQL condition that the `authorization_remote_grants` row named by
/// `grant` (its table name or alias) is live: the rule of
/// [`home::Liveness`] (#1013). Its lifetime bounds it until a receiver has
/// started its Task; from then on it lives while the Task has not ended,
/// however long the Run's Stint sleeps. Every Home check of a grant's
/// liveness uses this condition, so none of them reads `expires_at` alone.
pub(crate) fn live_condition(grant: &str) -> String {
	let ended = home::ending_statuses()
		.map(|status| format!("'{}'", status.as_str()))
		.collect::<Vec<_>>()
		.join(", ");
	format!(
		"(NOT {grant}.revoked AND ({grant}.expires_at > CLOCK_TIMESTAMP() OR EXISTS (SELECT 1 FROM authorization_remote_execution live_binding JOIN tasks live_task ON live_task.id = live_binding.task_id AND live_task.tenant = live_binding.tenant WHERE live_binding.grant_id = {grant}.id AND (live_binding.activated_at IS NOT NULL OR live_task.status <> '{open}') AND live_task.status NOT IN ({ended}))))",
		open = aidash_domain::TaskStatus::Open.as_str(),
	)
}

#[cfg(test)]
mod live_condition_tests {
	#[test]
	fn the_condition_names_the_grant_and_every_ending_status() {
		let condition = super::live_condition("g");
		assert!(condition.starts_with("(NOT g.revoked AND (g.expires_at > CLOCK_TIMESTAMP()"));
		assert!(condition.contains("live_binding.grant_id = g.id"));
		assert!(condition.contains("live_task.status <> 'OPEN'"));
		assert!(condition.contains("NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED')"));
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
