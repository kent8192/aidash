//! A source-owned workspace has no receiver-local workspace or task row.
//! Its receiving admission, not a coincidental local workspace UUID, owns the run.
use super::{
	Candidate, GraphActivity, GraphAuthority, GraphNode, GraphOptions, Run, event_reference,
	kind_allowed,
};
use crate::{
	Result,
	authorization::{access::Access, policy::Resource, remote::Description},
	domain::Event,
	federation::Federation,
};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{
	Alias, Expr, JoinType, LockType, Order, PostgresQueryBuilder, Query, SelectStatement,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn runs() -> SelectStatement {
	Query::select()
		.from_as(Alias::new("runs"), Alias::new("r"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("authorization_remote_admissions"),
			Alias::new("a"),
			Expr::cust("a.id = r.id AND a.source_node = r.home_node AND a.task_id = r.task_id"),
		)
		.and_where(Expr::cust(
			"a.tenant = $1 AND r.workspace_id = $2 AND r.home_node = $3",
		))
		.to_owned()
}

pub(super) async fn candidates(
	authority: &mut GraphAuthority<'_>,
	workspace: Uuid,
	source_node: &str,
	offset: u64,
) -> Result<Vec<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let rows: Vec<Run> = sqlx::query_as(
		&runs()
			.expr(Expr::cust("r.*"))
			.order_by((Alias::new("r"), Alias::new("id")), Order::Asc)
			.limit(super::CANDIDATE_BATCH)
			.offset(offset)
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.bind(workspace)
	.bind(source_node)
	.fetch_all(authority.connection())
	.await?;
	Ok(rows.into_iter().map(Candidate::Run).collect())
}

fn workspace_resource(access: &Access, source: &str, workspace: Uuid) -> Resource {
	// Match the receiving admission's qualified foreign resource namespace.
	access.resource(
		"workspace",
		format!("{source}/workspaces/{workspace}"),
		json!({}),
	)
}

pub(super) async fn visible(
	f: &Federation,
	authority: &mut GraphAuthority<'_>,
	run: &Run,
) -> Result<bool> {
	let tenant = authority.tenant().to_owned();
	let record: Option<(Uuid, Value)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("grant_id"), Alias::new("description")])
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::cust(
				"id = $1 AND tenant = $2 AND source_node = $3 AND task_id = $4",
			))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(tenant)
	.bind(&run.home_node)
	.bind(run.task_id)
	.fetch_optional(authority.connection())
	.await?;
	let Some((grant, value)) = record else {
		return Ok(false);
	};
	let description: Description = serde_json::from_value(value)?;
	if description.grant_id != grant
		|| description.source_node != run.home_node
		|| description.target_node != f.config.node_id
		|| description.task.id != run.task_id
		|| description.task.workspace_id != run.workspace_id
		|| description.inspection.node_id != f.config.node_id
		|| description.inspection.agent.kind != "agent"
		|| description.inspection.agent.id != run.agent_id
		|| description.inspection.agent.version != run.agent_version
	{
		return Ok(false);
	}
	let GraphAuthority::Subject(access) = authority else {
		return Ok(description.semantic.disabled());
	};
	let workspace = workspace_resource(access, &run.home_node, run.workspace_id);
	let task = access.resource(
		"task",
		format!("{}/tasks/{}", run.home_node, run.task_id),
		json!({"created_by":description.task.created_by,"requirements":description.task.requirements}),
	);
	let resource = access.resource("run", run.id, json!({}));
	let memory = access.resource(
		"memory",
		&run.agent_id,
		json!({
			"created_by":crate::domain::qualified_agent(&f.config.node_id, &run.agent_id, &run.agent_version),
			"version":run.agent_version,
		}),
	);
	// Admission identifies the data; only the current mapped viewer authorizes it.
	if !access.foreign_run_base_visible(run).await?
		|| !access.decide(&workspace, "workspace.read").await?
		|| !access.decide(&task, "task.read").await?
		|| !access.decide(&resource, "run.read").await?
		|| !access.decide(&memory, "memory.read").await?
		|| !access.run_reads_visible(run.id).await?
	{
		return Ok(false);
	}
	access.human_reads(run.workspace_id, run.id).await
}

pub(super) async fn revision(
	authority: &mut GraphAuthority<'_>,
	options: &GraphOptions,
	workspace: Uuid,
	source_node: &str,
	local_node: &str,
) -> Result<Value> {
	if !kind_allowed("run", options) {
		return Ok(Value::Null);
	}
	let tenant = authority.tenant().to_owned();
	let revisions: (Option<i64>, i64) = sqlx::query_as(
		&runs()
			.expr(Expr::cust("SUM(r.revision)::bigint"))
			.expr(Expr::cust("COUNT(*)"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&tenant)
	.bind(workspace)
	.bind(source_node)
	.fetch_one(authority.connection())
	.await?;
	let events: Option<i64> = sqlx::query_scalar(
		&runs()
			.expr(Expr::cust("MAX(e.sequence)"))
			.join_as(
				JoinType::InnerJoin,
				Alias::new("events"),
				Alias::new("e"),
				Expr::cust(
					"e.workspace_id IS NULL AND COALESCE(e.data->>'run_id', e.data->>'id') = r.id::text",
				),
			)
			.and_where(Expr::cust("e.node_id = $4 AND e.kind LIKE 'run.%'"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.bind(workspace)
	.bind(source_node)
	.bind(local_node)
	.fetch_one(authority.connection())
	.await?;
	Ok(json!({"runs":revisions,"events":events}))
}

pub(super) async fn activity(
	f: &Federation,
	authority: &mut GraphAuthority<'_>,
	options: &GraphOptions,
	nodes: &[GraphNode],
	window_end: DateTime<Utc>,
	source_node: &str,
	workspace: Uuid,
) -> Result<Vec<GraphActivity>> {
	let ids: Vec<String> = nodes
		.iter()
		.filter(|node| node.kind == "run")
		.filter_map(|node| node.resource_id.clone())
		.collect();
	if ids.is_empty() {
		return Ok(vec![]);
	}
	if let GraphAuthority::Subject(access) = authority {
		let resource = workspace_resource(access, source_node, workspace);
		if !access.decide(&resource, "workspace.events").await? {
			return Ok(vec![]);
		}
	}
	let mut query = Query::select();
	query
		.expr(Expr::cust("e.*"))
		.from_as(Alias::new("events"), Alias::new("e"))
		.and_where(Expr::cust("e.node_id = $1 AND e.workspace_id IS NULL"))
		.and_where(Expr::cust("e.kind LIKE 'run.%'"))
		.and_where(Expr::cust(
			"COALESCE(e.data->>'run_id', e.data->>'id') = ANY($2)",
		))
		.and_where(Expr::cust("e.created_at <= $3"))
		.order_by((Alias::new("e"), Alias::new("sequence")), Order::Desc)
		.limit(80);
	if options.hours > 0 {
		query.and_where(Expr::cust("e.created_at >= $4"));
	}
	let sql = query.to_string(PostgresQueryBuilder);
	let mut request = sqlx::query_as::<_, Event>(&sql)
		.bind(&f.config.node_id)
		.bind(ids)
		.bind(window_end);
	if options.hours > 0 {
		request = request.bind(window_end - chrono::Duration::hours(i64::from(options.hours)));
	}
	// These run nodes already passed admission binding and all current read gates.
	// Do not re-enter local-workspace authorization for their source-owned UUIDs.
	let events = request.fetch_all(authority.connection()).await?;
	Ok(events
		.into_iter()
		.filter_map(|event| {
			let reference = event_reference(&event, &f.config.node_id)?;
			nodes
				.iter()
				.any(|node| node.id == reference)
				.then_some(GraphActivity {
					kind: event.kind,
					at: event.created_at,
					reference,
				})
		})
		.collect())
}
