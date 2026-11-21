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

/// `owner` is the last subject of `chain`, the receiver's executor, which only that receiver's
/// claim makes a Task's owner: `chain[array_length(chain, 1)] = owner`. The query builder has
/// no array subscript, so it asks whether the chain's length is one of the positions `owner`
/// holds in it. A chain may name a subject twice (a hop back to an earlier receiver's Node),
/// and only its last position counts. An owner that is NULL or not in the chain holds no
/// position, and an empty chain has no length, so neither is held.
fn held_by_receiver(chain: SimpleExpr, owner: SimpleExpr) -> SimpleExpr {
	let function = |name: &str, arguments: Vec<SimpleExpr>| {
		SimpleExpr::FunctionCall(name.to_owned().into_iden(), arguments)
	};
	function(
		"array_length",
		vec![chain.clone(), Expr::value(1_i32).into()],
	)
	.eq(function(
		"ANY",
		vec![function("array_positions", vec![chain, owner])],
	))
}

/// A queued Run still has the original OPEN Task. Checking the grant's
/// original revision prevents a reopened Task from keeping an old grant live.
fn holds_or_awaits_task(grant: &Alias, binding: &Alias, task: &Alias) -> SimpleExpr {
	let column = |table: &Alias, name: &str| Expr::col((table.clone(), Alias::new(name)));
	let pending_claim = column(task, "status")
		.eq("OPEN")
		.and(column(task, "owner").is_null())
		.and(column(task, "revision").equals((grant.clone(), Alias::new("task_revision"))))
		.and(column(task, "revision").equals((binding.clone(), Alias::new("task_revision"))));
	held_by_receiver(
		column(grant, "subject_chain").into(),
		column(task, "owner").into(),
	)
	.and(column(task, "status").is_not_in(home::ending_statuses().map(|status| status.as_str())))
	.or(pending_claim)
}

/// The Run a grant row of `authorization_remote_grants` started is bound at the Home, has
/// started and has not ended. The binding recorded the receiver's activation, and
/// the receiver either still awaits its original Task or holds it (as [`live_condition`] requires). The
/// binding pins the Task revision the Run works on; a Task that has moved on to another revision
/// no longer has a live remote Run. The renewal of ADR 0109 (amended 2026-11-17) and the Home's
/// tool asks read it, so neither keeps a grant that [`live_condition`] would let expire.
pub(crate) fn run_live() -> SimpleExpr {
	let (binding, task) = (Alias::new("b"), Alias::new("t"));
	let column = |table: &Alias, name: &str| Expr::col((table.clone(), Alias::new(name)));
	let grants = Alias::new("authorization_remote_grants");
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
			.and_where(column(&binding, "grant_id").equals((grants.clone(), Alias::new("id"))))
			.and_where(column(&binding, "activated_at").is_not_null())
			.and_where(holds_or_awaits_task(&grants, &binding, &task))
			.to_owned(),
	)
	.into()
}

/// The condition that the `authorization_remote_grants` row named by `grant`
/// (its table name or alias) is live: the rule of [`home::Liveness`] (#1013).
/// Its lifetime bounds it until this grant's own Home binding has recorded the
/// receiver's activation. From then on it lives while the original Task is
/// still awaiting its first claim, or while the receiver holds the unfinished Task.
/// Every Home check of a grant's liveness uses this condition, so none of them
/// reads `expires_at` alone.
pub(crate) fn live_condition(grant: &str) -> SimpleExpr {
	let grant = Alias::new(grant);
	let (binding, task) = (Alias::new("live_binding"), Alias::new("live_task"));
	let column = |table: &Alias, name: &str| Expr::col((table.clone(), Alias::new(name)));
	let held = Query::select()
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
				.and(column(&task, "tenant").equals((binding.clone(), Alias::new("tenant")))),
		)
		.and_where(column(&binding, "grant_id").equals((grant.clone(), Alias::new("id"))))
		.and_where(column(&binding, "task_id").equals((grant.clone(), Alias::new("task_id"))))
		.and_where(column(&binding, "activated_at").is_not_null())
		.and_where(holds_or_awaits_task(&grant, &binding, &task))
		.to_owned();
	column(&grant, "revoked").not().and(
		column(&grant, "expires_at")
			.gt(clock_timestamp())
			.or(Expr::exists(held)),
	)
}

#[cfg(test)]
mod live_condition_tests {
	use reinhardt::query::{Alias, PostgresQueryBuilder, Query, QueryStatementBuilder as _};

	fn rendered(condition: reinhardt::query::SimpleExpr) -> String {
		Query::select()
			.expr(condition)
			.from_as(Alias::new("authorization_remote_grants"), Alias::new("g"))
			.to_string(PostgresQueryBuilder)
	}

	/// A live grant has time left or a recorded activation for a Task the
	/// receiver holds or has yet to claim. Revocation takes precedence.
	#[test]
	fn the_condition_needs_this_grants_activation_its_executor_and_an_open_task() {
		assert_eq!(
			rendered(super::live_condition("g")),
			"SELECT NOT \"g\".\"revoked\" AND (\"g\".\"expires_at\" > CLOCK_TIMESTAMP() \
			OR EXISTS (SELECT TRUE FROM \"authorization_remote_execution\" AS \"live_binding\" \
			INNER JOIN \"tasks\" AS \"live_task\" ON \"live_task\".\"id\" = \"live_binding\".\"task_id\" \
			AND \"live_task\".\"tenant\" = \"live_binding\".\"tenant\" \
			WHERE \"live_binding\".\"grant_id\" = \"g\".\"id\" \
			AND \"live_binding\".\"task_id\" = \"g\".\"task_id\" \
			AND \"live_binding\".\"activated_at\" IS NOT NULL \
			AND (array_length(\"g\".\"subject_chain\", 1) = ANY(array_positions(\"g\".\"subject_chain\", \"live_task\".\"owner\")) \
			AND \"live_task\".\"status\" NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED') \
			OR \"live_task\".\"status\" = 'OPEN' AND \"live_task\".\"owner\" IS NULL \
			AND \"live_task\".\"revision\" = \"g\".\"task_revision\" AND \"live_task\".\"revision\" = \"live_binding\".\"task_revision\"))) \
			FROM \"authorization_remote_grants\" AS \"g\""
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
