//! Database rows are converted explicitly into portable source execution state.
use aidash_domain::federation::execution::home;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, IntoIden as _, JoinType, Query, SimpleExpr, TableRef,
};
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

/// `CLOCK_TIMESTAMP()`: the time the statement reads it, not the transaction's start as
/// `CURRENT_TIMESTAMP` would be, so a long transaction never takes an expired grant for live.
pub(crate) fn clock_timestamp() -> SimpleExpr {
	SimpleExpr::FunctionCall("CLOCK_TIMESTAMP".into_iden(), Vec::new())
}

/// A grant row of `authorization_remote_grants` that is neither revoked nor expired.
pub(crate) fn unexpired() -> SimpleExpr {
	Expr::col(Alias::new("revoked"))
		.not()
		.and(Expr::col(Alias::new("expires_at")).gt(clock_timestamp()))
}

/// The Run a grant row of `authorization_remote_grants` started is bound at the Home and has
/// not ended. The binding pins the Task revision the Run works on; a Task that has moved on to
/// another revision, or reached a terminal status, no longer has a live remote Run.
pub(crate) fn run_live() -> SimpleExpr {
	let (binding, task) = (Alias::new("b"), Alias::new("t"));
	let column = |table: &Alias, name: &str| Expr::col((table.clone(), Alias::new(name)));
	Expr::exists(
		Query::select()
			.expr(Expr::constant_true())
			.from_as(
				Alias::new("authorization_remote_execution"),
				binding.clone(),
			)
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("tasks"), task.clone()),
				column(&task, "id")
					.equals((binding.clone(), Alias::new("task_id")))
					.and(
						column(&task, "revision")
							.equals((binding.clone(), Alias::new("task_revision"))),
					),
			)
			.and_where(
				column(&binding, "grant_id")
					.equals((Alias::new("authorization_remote_grants"), Alias::new("id"))),
			)
			.and_where(column(&task, "status").is_not_in([
				"OPEN",
				"COMPLETED",
				"FAILED",
				"CANCELLED",
				"ABANDONED",
			]))
			.to_owned(),
	)
	.into()
}

pub(crate) mod authority;

pub(crate) mod persistence;

pub(crate) mod semantic;

pub(crate) mod reads;

pub(crate) mod provenance;

pub(crate) mod search;

pub(crate) mod snapshot;
