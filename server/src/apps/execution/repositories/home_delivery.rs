//! Where a Home puts what a Run executing for another Home delivers (ADR 0139, decision 20).
//!
//! The grant records who may act, not which Run delegated. The delegating Run is the latest
//! Run of the grant task's parent that is the grant chain's delegating Agent: a Run executes a
//! Task it created children for, and a grant is made for a child. When no such Run or Working
//! Area exists the delivery is refused by name instead of landing somewhere else.
use crate::{Error, Result, authorization::access::Access};
use aidash_application::ports::capabilities::sessions::SessionScope as _;
use aidash_domain::capabilities::sessions::Area;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use uuid::Uuid;

/// The Working Area of the Run that delegated, and that Run's Agent version. `delegator` is the
/// chain's subject just before the receiving executor, on `deployment`.
pub(crate) async fn delegating_area(
	access: &mut Access,
	deployment: &str,
	parent: Uuid,
	delegator: &str,
) -> Result<(Area, String)> {
	let refused = || Error::NotFound("DELIVERY_NO_DELEGATING_RUN".into());
	let (node, rest) = delegator.rsplit_once("/agents/").ok_or_else(refused)?;
	let (agent, version) = rest.rsplit_once('@').ok_or_else(refused)?;
	if !node.starts_with(deployment) {
		return Err(refused());
	}
	let run: Option<Uuid> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col(Alias::new("task_id")).eq(Expr::value(parent)))
			.and_where(Expr::col(Alias::new("agent_id")).eq(agent))
			.and_where(Expr::col(Alias::new("agent_version")).eq(version))
			.order_by(Alias::new("created_at"), Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **access.tx)
	.await?;
	let run = run.ok_or_else(refused)?;
	let area = crate::bootstrap::session_scope(None, access)
		.context_area(run)
		.await?
		.ok_or_else(|| Error::NotFound("DELIVERY_NO_DELEGATING_AREA".into()))?;
	Ok((area, version.to_owned()))
}
