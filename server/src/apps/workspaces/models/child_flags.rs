//! What the final-answer check needs to know about an open child Task: whether
//! anyone has started it and whether a Run of its parent created it (#772).
//!
//! Every reader of a child summary selects these as correlated `EXISTS`
//! expressions over `tasks`, so the local store, the guarded read and the
//! Node-to-Node read cannot disagree about a Task, and one statement answers
//! for a whole page of children.
use reinhardt::query::{Alias, Expr, ExprTrait, Query, SimpleExpr, TableRef};
use uuid::Uuid;

fn this_task() -> (Alias, Alias) {
	(Alias::new("tasks"), Alias::new("id"))
}

/// Somebody has the Task: a Run of it is under way, another Node confirmed it
/// started it, or a generated specialist is still coming for it.
///
/// A finished Run, a cancelled Run and the binding of a Run that ended before
/// the Task was reopened do not count. The binding is live only while it
/// carries the Task's current revision: every command of the remote Run
/// brings the binding up to the Task, and a reopening moves the Task past it.
pub(crate) fn started() -> SimpleExpr {
	let run = Query::select()
		.expr(Expr::value(1_i64))
		.from(Alias::new("runs"))
		.and_where(Expr::col("task_id").equals(this_task()))
		.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
		.and_where(Expr::col("control").ne("CANCELLED"))
		.to_owned();
	let other_node = Query::select()
		.expr(Expr::value(1_i64))
		.from(Alias::new("authorization_remote_execution"))
		.and_where(Expr::col("task_id").equals(this_task()))
		.and_where(Expr::col("activated_at").is_not_null())
		.and_where(Expr::col("task_revision").equals((Alias::new("tasks"), Alias::new("revision"))))
		.to_owned();
	// PENDING_APPROVAL, QUEUED and ACTIVE are the open states of a request.
	let specialist = Query::select()
		.expr(Expr::value(1_i64))
		.from(Alias::new("generation_requests"))
		.and_where(Expr::col("task_id").equals(this_task()))
		.and_where(Expr::col("status").is_in(["PENDING_APPROVAL", "QUEUED", "ACTIVE"]))
		.to_owned();
	Expr::exists(run)
		.or(Expr::exists(other_node))
		.or(Expr::exists(specialist))
}

/// A Run of `parent` created the Task, here or on another Node, and nothing
/// open sits beneath it. Abandoning a Task with open children is refused, so
/// a Task with children goes to a person instead of failing the completion.
///
/// A person's Task has no origin row, so it is never abandonable. A Run on
/// another Node leaves no origin row either: the Home records what it created
/// in `authorization_remote_outputs`, under the grant of that execution.
pub(crate) fn abandonable(parent: Uuid) -> SimpleExpr {
	let parent_runs = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("runs"))
		.and_where(Expr::col("task_id").eq(Expr::value(parent)))
		.to_owned();
	let local = Query::select()
		.expr(Expr::value(1_i64))
		.from(Alias::new("authorization_task_origins"))
		.and_where(Expr::col("task_id").equals(this_task()))
		.and_where(Expr::col("source_run_id").in_subquery(parent_runs))
		.to_owned();
	let parent_grants = Query::select()
		.column(Alias::new("grant_id"))
		.from(Alias::new("authorization_remote_execution"))
		.and_where(Expr::col("task_id").eq(Expr::value(parent)))
		.to_owned();
	let remote = Query::select()
		.expr(Expr::value(1_i64))
		.from(Alias::new("authorization_remote_outputs"))
		.and_where(Expr::col("resource_kind").eq("task"))
		.and_where(Expr::col("resource_id").equals(this_task()))
		.and_where(Expr::col("grant_id").in_subquery(parent_grants))
		.to_owned();
	let open_child = Query::select()
		.expr(Expr::value(1_i64))
		.from(TableRef::table_alias(
			Alias::new("tasks"),
			Alias::new("child"),
		))
		.and_where(Expr::col((Alias::new("child"), Alias::new("parent_id"))).equals(this_task()))
		.and_where(
			Expr::col((Alias::new("child"), Alias::new("status")))
				.is_not_in(["COMPLETED", "ABANDONED"]),
		)
		.to_owned();
	Expr::exists(local)
		.or(Expr::exists(remote))
		.and(Expr::not_exists(open_child))
}

/// Open and not started: the first condition of an Unassigned Task.
pub(crate) fn unstarted_open() -> SimpleExpr {
	Expr::col((Alias::new("tasks"), Alias::new("status")))
		.eq("OPEN")
		.and(started().not())
}

/// Only an open or claimed child can be unassigned, so only those rows pay
/// for the started and abandonable subqueries.
pub(crate) fn open_or_claimed() -> SimpleExpr {
	Expr::col((Alias::new("tasks"), Alias::new("status"))).is_in(["OPEN", "CLAIMED"])
}
