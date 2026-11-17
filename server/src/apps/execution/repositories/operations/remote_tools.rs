//! Whether a Run executing here for another Home may still use the tool its work depends on
//! (ADR 0139, decision 12). The 30 second Read Recheck holds no Home connection, so it decides
//! from what this Node keeps: its own registration of the Home (Allowance and Block), the
//! Run-only answers on the admission and the Home's Request as the admission last saw it. A
//! change of the Home's Request that the Home has not yet sent reaches a running command at the
//! Run's next worker boundary, where the lease asks the Home afresh.
use crate::{Error, Result as NativeResult};
use aidash_domain::{
	federation::{
		execution::Description,
		tool_lists::{OwnedRemoteToolLists, ToolList},
	},
	nodes::NodeAddress,
};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{QueryRow, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// How far up a Subnode chain a Run's lists are looked for: a member may itself create one.
const MAXIMUM_DEPTH: usize = 8;

/// The Run whose lists bound `run`'s work. A Subnode member's Run uses at most what the Run
/// that created its Subnode may use, so it is bounded by that creator, and by the creator's own
/// creator when that one is a member too. Any other Run is its own.
pub(crate) async fn bounding_run(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> NativeResult<Uuid> {
	let mut current = run;
	for _ in 0..MAXIMUM_DEPTH {
		let (sql, values) = Query::select()
			.column(Alias::new("creator_run_id"))
			.from(Alias::new("subnodes"))
			.and_where(
				Expr::col("workspace_id").in_subquery(
					Query::select()
						.column(Alias::new("workspace_id"))
						.from(Alias::new("runs"))
						.and_where(Expr::col("id").eq(Expr::value(current)))
						.to_owned(),
				),
			)
			.build(PostgresQueryBuilder);
		let creator: Option<Uuid> = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.and_then(|row| QueryRow::from_backend_row(row).get("creator_run_id"));
		match creator {
			Some(creator) if creator != current => current = creator,
			_ => break,
		}
	}
	Ok(current)
}

/// Whether this Node executes `run` for another Home, that is, holds an admission for it.
pub(crate) async fn is_admitted(tx: &mut dyn TransactionExecutor, run: Uuid) -> NativeResult<bool> {
	Ok(
		crate::apps::federation::peer::repositories::admissions::for_run(tx, run)
			.await?
			.is_some(),
	)
}

/// The lists that decide `run`'s tools, from what this Node holds. `None` when the Run has no
/// admission here, or its Home shares this Tenant and no lists apply. A Home that asks for
/// tools but that this Node holds no registration of is refused.
pub(crate) async fn lists(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> NativeResult<Option<OwnedRemoteToolLists>> {
	let Some(admission) =
		crate::apps::federation::peer::repositories::admissions::for_run(&mut *tx, run).await?
	else {
		return Ok(None);
	};
	let description: Description = serde_json::from_value(admission.description.0.clone())?;
	let request =
		aidash_application::authorization::peer::tool_request::requested_tools(&description)
			.map_err(Error::from)?;
	let home = NodeAddress::parse(&admission.source_node).map_err(|_| Error::Forbidden)?;
	let registration = crate::apps::identity::repositories::node_registrations::get(
		&mut *tx,
		&admission.tenant,
		&admission.node,
		&home,
	)
	.await?;
	let Some(registration) = registration else {
		return match request {
			None => Ok(None),
			Some(_) => Err(Error::Forbidden),
		};
	};
	let run_allowance: ToolList = match admission.run_tool_allowance {
		Some(stored) => serde_json::from_value(stored.0)?,
		None => ToolList::default(),
	};
	Ok(Some(OwnedRemoteToolLists {
		request: request.map(|request| request.tools).unwrap_or_default(),
		allowance: registration.tool_allowance,
		block: registration.tool_block,
		// The Home's Run-only answers already travel inside its Request.
		run_request: ToolList::default(),
		run_allowance,
	}))
}

/// Whether the work `run` has going with `tool` must stop. A Run of this Node's own Home is
/// never stopped by lists; a Subnode member's Run answers to the lists of the Run that created
/// its Subnode.
pub(crate) async fn withdrawn(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	tool: &str,
) -> NativeResult<bool> {
	let bound = bounding_run(&mut *tx, run).await?;
	let lists = lists(tx, bound).await?;
	Ok(aidash_application::authorization::tools::tool_withdrawn(
		lists.as_ref(),
		tool,
	))
}
