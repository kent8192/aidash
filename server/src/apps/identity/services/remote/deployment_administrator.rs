//! Installation administration is not a mapped tenant reader at another node.
//! Content in a workspace bound to required Home retrieval requires a Subject
//! view. Deployment Administrator APIs retain content-free stop controls and infrastructure data.
use crate::database::native::Transaction;
use crate::{Error, Result, domain::Event};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, JoinType, PostgresQueryBuilder, Query, SimpleExpr,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn deleted_tenants() -> reinhardt::query::SelectStatement {
	Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new("tenant_deletions"))
		.and_where(Expr::col("restored_at").is_null())
		.to_owned()
}

/// Deletion takes the exclusive policy lock. The caller retains this shared
/// lock through its protected read or write.
pub(crate) async fn tenant(
	connection: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<()> {
	let result = crate::apps::identity::models::AuthorizationBundle::lock_snapshot(
		connection, tenant, false,
	)
	.await;
	// A purge removes the policy, but retains the deleted Tenant's tombstone.
	if matches!(&result, Err(Error::NotFound(_)))
		&& crate::apps::identity::repositories::tenant_deletions::is_deleted(connection, tenant)
			.await?
	{
		return Err(Error::TenantDeleted);
	}
	result.map(|_| ())
}

/// Preserve the deleted-Tenant HTTP response at public administration ports.
/// Worker ports keep their existing conversion to refused authority.
pub(crate) fn application_error(error: Error) -> aidash_application::Error {
	match error {
		error @ Error::TenantDeleted => aidash_application::Error::Port(Box::new(error)),
		error => error.into(),
	}
}

/// Index configuration is allowed for required-Home Workspaces, but never for
/// a deleted Tenant. The same policy lock protects configuration and its effects.
pub(crate) async fn workspace_tenant(connection: &mut Transaction, workspace: Uuid) -> Result<()> {
	// A receiver Run may name a foreign Workspace with no local Workspace row.
	// Its already resolved transaction scope still identifies the owning Tenant.
	if let Some(owner) = connection.scope().and_then(|scope| scope.tenant_name()) {
		let owner = owner.as_str().to_owned();
		return tenant(&mut **connection, &owner).await;
	}
	let query = Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new("workspaces"))
		.and_where(Expr::col("id").eq(Expr::value(workspace)))
		.to_string(PostgresQueryBuilder);
	let owner: String = crate::database::native::query_scalar(&query)
		.scalar_one(&mut *connection)
		.await?;
	tenant(&mut **connection, &owner).await
}

async fn deleted_workspaces(
	connection: &mut Transaction,
	workspaces: &[Uuid],
) -> Result<BTreeSet<Uuid>> {
	if workspaces.is_empty() {
		return Ok(BTreeSet::new());
	}
	let query = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("workspaces"))
		.and_where(Expr::col("id").is_in(workspaces.iter().copied().map(Expr::value)))
		.and_where(Expr::col("tenant").in_subquery(deleted_tenants()))
		.to_string(PostgresQueryBuilder);
	let ids: Vec<Uuid> = crate::database::native::query_scalar(&query)
		.scalar_all(&mut *connection)
		.await?;
	Ok(ids.into_iter().collect())
}

/// Tenant events need this check even when they name no local Workspace or Run.
async fn deleted_events(connection: &mut Transaction, events: &[Event]) -> Result<BTreeSet<Uuid>> {
	if events.is_empty() {
		return Ok(BTreeSet::new());
	}
	let query = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("events"))
		.and_where(Expr::col("id").is_in(events.iter().map(|event| Expr::value(event.id))))
		.and_where(Expr::col("tenant").in_subquery(deleted_tenants()))
		.to_string(PostgresQueryBuilder);
	let ids: Vec<Uuid> = crate::database::native::query_scalar(&query)
		.scalar_all(&mut *connection)
		.await?;
	Ok(ids.into_iter().collect())
}

pub(crate) async fn blocked(
	connection: &mut Transaction,
	workspaces: &[Uuid],
) -> Result<BTreeSet<Uuid>> {
	if workspaces.is_empty() {
		return Ok(BTreeSet::new());
	}
	let receiver = Query::select()
		.column((Alias::new("r"), Alias::new("workspace_id")))
		.from_as(Alias::new("runs"), Alias::new("r"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("authorization_remote_admissions"),
				Alias::new("a"),
			),
			Expr::cust("a.id=r.id"),
		)
		.and_where(Expr::cust(
			"r.workspace_id=ANY($1) AND a.description->'semantic'->>'mode'='required_home'",
		))
		.to_owned();
	let query = Query::select()
		.column((Alias::new("t"), Alias::new("workspace_id")))
		.from_as(Alias::new("tasks"), Alias::new("t"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("authorization_remote_grants"),
				Alias::new("g"),
			),
			Expr::cust("g.task_id=t.id"),
		)
		.and_where(Expr::cust(
			"t.workspace_id=ANY($1) AND g.semantic->>'mode'='required_home'",
		))
		.union(receiver)
		.to_string(PostgresQueryBuilder);
	let values: Vec<Uuid> = crate::database::native::query_scalar(&query)
		.bind(workspaces)
		.scalar_all(&mut *connection)
		.await?;
	let mut denied: BTreeSet<_> = values.into_iter().collect();
	denied.extend(deleted_workspaces(connection, workspaces).await?);
	Ok(denied)
}

pub(crate) async fn visible(connection: &mut Transaction, workspace: Uuid) -> Result<bool> {
	Ok(blocked(connection, &[workspace]).await?.is_empty())
}

pub(crate) async fn require(connection: &mut Transaction, workspace: Uuid) -> Result<()> {
	workspace_tenant(connection, workspace).await?;
	if visible(connection, workspace).await? {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

pub(crate) async fn event_visible(connection: &mut Transaction, event: &Event) -> Result<bool> {
	if deleted_events(connection, std::slice::from_ref(event))
		.await?
		.contains(&event.id)
	{
		return Ok(false);
	}
	if let Some(workspace) = event_workspaces(connection, std::slice::from_ref(event)).await?[0] {
		return visible(connection, workspace).await;
	}
	Ok(true)
}

async fn event_workspaces(
	connection: &mut Transaction,
	events: &[Event],
) -> Result<Vec<Option<Uuid>>> {
	// Foreign Run events have no local Workspace FK. Resolve the durable Run
	// binding instead of treating a NULL event.workspace_id as global content.
	let run_id = |event: &Event| {
		event
			.data
			.get("run_id")
			.or_else(|| {
				(event.kind == "run.created")
					.then(|| event.data.get("id"))
					.flatten()
			})
			.and_then(serde_json::Value::as_str)
			.and_then(|id| id.parse::<Uuid>().ok())
	};
	let ids: Vec<_> = events.iter().filter_map(run_id).collect();
	let runs: Vec<(Uuid, Uuid)> = {
		let query_bind_1 = &ids;
		crate::database::native::query_as(
			&Query::select()
				.columns(["id", "workspace_id"].map(Alias::new))
				.from(Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=ANY(?))".to_owned(),
					vec![crate::database::uuid_array(query_bind_1.to_owned())],
				))
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["id", "workspace_id"])
		.fetch_all(connection)
		.await?
	};
	let runs: BTreeMap<_, _> = runs.into_iter().collect();
	Ok(events
		.iter()
		.map(|event| {
			event
				.workspace_id
				.or_else(|| run_id(event).and_then(|id| runs.get(&id).copied()))
		})
		.collect())
}

use reinhardt::query::QueryStatementBuilder as _;

/// Apply the required-Home exclusion before a state collection's LIMIT.
/// `column` is a fixed query alias supplied by the caller.
pub(crate) fn state_visible(column: &str) -> SimpleExpr {
	let deleted = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("workspaces"))
		.and_where(Expr::col("tenant").in_subquery(deleted_tenants()))
		.to_owned();
	Expr::cust(format!(
		"NOT EXISTS (SELECT 1 FROM tasks v_task JOIN authorization_remote_grants v_grant ON v_grant.task_id=v_task.id WHERE v_task.workspace_id={column} AND v_grant.semantic->>'mode'='required_home') AND NOT EXISTS (SELECT 1 FROM runs v_run JOIN authorization_remote_admissions v_admission ON v_admission.id=v_run.id WHERE v_run.workspace_id={column} AND v_admission.description->'semantic'->>'mode'='required_home')"
	)).and(Expr::cust(column).not_in_subquery(deleted))
}

pub(crate) async fn filter_events(
	connection: &mut Transaction,
	events: Vec<Event>,
) -> Result<Vec<Event>> {
	let deleted = deleted_events(connection, &events).await?;
	let workspaces = event_workspaces(connection, &events).await?;
	let denied = blocked(
		connection,
		&workspaces.iter().flatten().copied().collect::<Vec<_>>(),
	)
	.await?;
	Ok(events
		.into_iter()
		.zip(workspaces)
		.filter_map(|(event, workspace)| {
			if deleted.contains(&event.id) {
				return None;
			}
			workspace
				.is_none_or(|id| !denied.contains(&id))
				.then_some(event)
		})
		.collect())
}

pub(crate) async fn filter_state(
	connection: &mut Transaction,
	state: &mut crate::apps::execution::serializers::state::StateResponse,
) -> Result<()> {
	let ids = state
		.workspaces
		.iter()
		.map(|w| w.id)
		.chain(state.runs.iter().map(|r| r.workspace_id))
		.chain(state.events.iter().filter_map(|e| e.workspace_id))
		.chain(state.tasks.iter().map(|t| t.workspace_id))
		.chain(state.artifacts.iter().map(|a| a.workspace_id))
		.chain(state.human_requests.iter().map(|h| h.workspace_id))
		.chain(state.conversations.iter().map(|c| c.workspace_id))
		.collect::<Vec<_>>();
	let denied = blocked(connection, &ids).await?;
	state.workspaces.retain(|w| !denied.contains(&w.id));
	state.tasks.retain(|t| !denied.contains(&t.workspace_id));
	state.runs.retain(|r| !denied.contains(&r.workspace_id));
	state
		.artifacts
		.retain(|a| !denied.contains(&a.workspace_id));
	state
		.human_requests
		.retain(|h| !denied.contains(&h.workspace_id));
	state
		.conversations
		.retain(|c| !denied.contains(&c.workspace_id));
	state.events = filter_events(connection, std::mem::take(&mut state.events)).await?;
	Ok(())
}
