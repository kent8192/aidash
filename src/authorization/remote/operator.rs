//! Installation administration is not a mapped tenant reader at another node.
//! Content in a workspace bound to required Home retrieval requires a Subject
//! view. Operator APIs retain content-free stop controls and infrastructure data.
use crate::{Error, Result, domain::Event};
use sea_orm::sea_query::{
	Alias, Expr, JoinType, PostgresQueryBuilder, Query, SimpleExpr, UnionType,
};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Apply the required-Home exclusion before a state collection's LIMIT.
/// `column` is a fixed query alias supplied by the caller.
pub(crate) fn state_visible(column: &str) -> SimpleExpr {
	Expr::cust(format!(
		"NOT EXISTS (SELECT 1 FROM tasks v_task JOIN authorization_remote_grants v_grant ON v_grant.task_id=v_task.id WHERE v_task.workspace_id={column} AND v_grant.semantic->>'mode'='required_home') AND NOT EXISTS (SELECT 1 FROM runs v_run JOIN authorization_remote_admissions v_admission ON v_admission.id=v_run.id WHERE v_run.workspace_id={column} AND v_admission.description->'semantic'->>'mode'='required_home')"
	))
}

pub(crate) async fn blocked(
	connection: &mut PgConnection,
	workspaces: &[Uuid],
) -> Result<BTreeSet<Uuid>> {
	if workspaces.is_empty() {
		return Ok(BTreeSet::new());
	}
	let receiver = Query::select()
		.column((Alias::new("r"), Alias::new("workspace_id")))
		.from_as(Alias::new("runs"), Alias::new("r"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("authorization_remote_admissions"),
			Alias::new("a"),
			Expr::cust("a.id=r.id"),
		)
		.and_where(Expr::cust(
			"r.workspace_id=ANY($1) AND a.description->'semantic'->>'mode'='required_home'",
		))
		.to_owned();
	let query = Query::select()
		.column((Alias::new("t"), Alias::new("workspace_id")))
		.from_as(Alias::new("tasks"), Alias::new("t"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("authorization_remote_grants"),
			Alias::new("g"),
			Expr::cust("g.task_id=t.id"),
		)
		.and_where(Expr::cust(
			"t.workspace_id=ANY($1) AND g.semantic->>'mode'='required_home'",
		))
		.union(UnionType::Distinct, receiver)
		.to_string(PostgresQueryBuilder);
	let values: Vec<Uuid> = sqlx::query_scalar(&query)
		.bind(workspaces)
		.fetch_all(connection)
		.await?;
	Ok(values.into_iter().collect())
}

pub(crate) async fn visible(connection: &mut PgConnection, workspace: Uuid) -> Result<bool> {
	Ok(blocked(connection, &[workspace]).await?.is_empty())
}

pub(crate) async fn require(connection: &mut PgConnection, workspace: Uuid) -> Result<()> {
	if visible(connection, workspace).await? {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

pub(crate) async fn event_visible(connection: &mut PgConnection, event: &Event) -> Result<bool> {
	if let Some(workspace) = event_workspaces(connection, std::slice::from_ref(event)).await?[0] {
		return visible(connection, workspace).await;
	}
	Ok(true)
}

async fn event_workspaces(
	connection: &mut PgConnection,
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
	let runs: Vec<(Uuid, Uuid)> = sqlx::query_as(
		&Query::select()
			.columns(["id", "workspace_id"].map(Alias::new))
			.from(Alias::new("runs"))
			.and_where(Expr::cust("id=ANY($1)"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&ids)
	.fetch_all(connection)
	.await?;
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

pub(crate) async fn filter_events(
	connection: &mut PgConnection,
	events: Vec<Event>,
) -> Result<Vec<Event>> {
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
			workspace
				.is_none_or(|id| !denied.contains(&id))
				.then_some(event)
		})
		.collect())
}

pub(crate) async fn filter_state(
	connection: &mut PgConnection,
	state: &mut crate::api_schema::StateResponse,
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
