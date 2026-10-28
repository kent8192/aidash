//! Native graph reads retain the caller policy, peer mapping and grant locks.
mod catalog;
mod scoped;
use crate::authorization::access::Access;
use crate::database::native::Transaction;
use crate::{
	Error, Result,
	domain::{Artifact, Conversation, Event, Task, Workspace},
	federation::Federation,
	registry::Entry,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reinhardt::query::{Alias, Condition, Expr, JoinType, Order, PostgresQueryBuilder, Query};
use serde_json::json;
use sha2::Sha256;
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::domain::RunMetadata;
use aidash_domain::federation::graph::*;
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _, SimpleExpr};
/// Holds the sender's registration of `node` until the transaction ends.
pub(crate) async fn source_peer_lease(
	tx: &mut crate::database::native::Transaction,
	own: &aidash_domain::nodes::DeploymentId,
	sender: &aidash_domain::federation::route::Sender,
	node: &str,
) -> Result<()> {
	crate::apps::federation::remote::repositories::routes::route_to(tx, own, sender, node, true)
		.await?
		.map(|_| ())
		.ok_or(Error::Forbidden)
}

/// The receiving Tenant's Subject, whose current policy gates every page.
pub(crate) struct GraphAuthority<'a>(pub(crate) &'a mut Access);

impl GraphAuthority<'_> {
	pub(super) fn tenant(&self) -> &str {
		&self.0.identity.tenant
	}

	pub(super) fn connection(&mut self) -> &mut Transaction {
		&mut self.0.tx
	}

	pub(crate) async fn visible(&mut self, candidate: &Candidate) -> Result<bool> {
		aidash_application::federation::graph::visibility::visible(
			&mut crate::bootstrap::graph_visibility(self),
			candidate,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn workspace_allowed(&mut self, id: Uuid, action: &str) -> Result<bool> {
		let resource = self.0.workspace(id).await?;
		self.0.decide(&resource, action).await
	}

	async fn event_visible(&mut self, event: &Event) -> Result<bool> {
		// project_activity checks workspace.events once per workspace. Retain
		// the event-specific resource and provenance checks without repeating it.
		self.0.event_visible(event).await
	}
}

const ACTIVITY_SCAN_LIMIT: usize = 4096;

pub(crate) async fn candidates(
	authority: &mut GraphAuthority<'_>,
	kind: u8,
	offset: u64,
	options: &GraphOptions,
	source_node: &str,
) -> Result<Vec<Candidate>> {
	let tenant = authority.tenant().to_owned();
	if options.scope_workspace.is_some() && kind != 3 && kind != 5 {
		return Ok(vec![]);
	}
	if kind == 0 && !kind_allowed("workspace", options) && !kind_allowed("goal", options) {
		return Ok(vec![]);
	}
	if kind == 1 && !kind_allowed("task", options) {
		return Ok(vec![]);
	}
	if kind == 2 && !kind_allowed("artifact", options) {
		return Ok(vec![]);
	}
	if kind == 3 && !kind_allowed("run", options) {
		return Ok(vec![]);
	}
	if kind == 4 && !kind_allowed("conversation", options) {
		return Ok(vec![]);
	}
	if kind == 3
		&& let Some(workspace) = options.scope_workspace
	{
		return scoped::candidates(authority, workspace, source_node, offset).await;
	}
	if kind == 5 {
		return catalog::candidates(authority, options, offset).await;
	}
	let conn = authority.connection();
	let table = match kind {
		0 => "workspaces",
		1 => "tasks",
		2 => "artifacts",
		3 => "runs",
		4 => "conversations",
		_ => unreachable!(),
	};
	let join_column = if kind == 0 { "id" } else { "workspace_id" };
	let sql = Query::select()
		.column(reinhardt::query::ColumnRef::table_asterisk("r"))
		.from_as(Alias::new(table), Alias::new("r"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("authorization_workspaces"),
				Alias::new("a"),
			),
			reinhardt::query::SimpleExpr::from(Expr::col((
				Alias::new("r"),
				Alias::new(join_column),
			)))
			.eq(Expr::col((Alias::new("a"), Alias::new("workspace_id")))),
		)
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col((Alias::new("a"), Alias::new("tenant"))))
				.eq(Expr::value(&tenant)),
		)
		.order_by((Alias::new("r"), Alias::new("id")), Order::Asc)
		.limit(CANDIDATE_BATCH)
		.offset(offset)
		.to_string(PostgresQueryBuilder);
	match kind {
		0 => Ok(aidash_server::database::query_as::<Workspace>(&sql)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Workspace)
			.collect()),
		1 => Ok(aidash_server::database::query_as::<Task>(&sql)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Task)
			.collect()),
		2 => Ok(aidash_server::database::query_as::<Artifact>(&sql)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Artifact)
			.collect()),
		3 => Ok(aidash_server::database::query_as::<RunMetadata>(&sql)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Run)
			.collect()),
		4 => Ok(aidash_server::database::query_as::<Conversation>(&sql)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Conversation)
			.collect()),
		_ => unreachable!(),
	}
}

pub(crate) async fn linked_workspace(
	authority: &mut GraphAuthority<'_>,
	id: Uuid,
) -> Result<Option<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let sql = tenant_resources("workspaces", "id", &tenant)
		.expr(Expr::cust("r.*"))
		.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).eq(Expr::value(id)))
		.limit(1)
		.to_string(PostgresQueryBuilder);
	Ok(aidash_server::database::query_as::<Workspace>(&sql)
		.fetch_optional(authority.connection())
		.await?
		.map(Candidate::Workspace))
}

pub(crate) async fn linked_task(
	authority: &mut GraphAuthority<'_>,
	id: Uuid,
) -> Result<Option<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let sql = tenant_resources("tasks", "workspace_id", &tenant)
		.expr(Expr::cust("r.*"))
		.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).eq(Expr::value(id)))
		.limit(1)
		.to_string(PostgresQueryBuilder);
	Ok(aidash_server::database::query_as::<Task>(&sql)
		.fetch_optional(authority.connection())
		.await?
		.map(Candidate::Task))
}

pub(crate) async fn linked_registry(
	authority: &mut GraphAuthority<'_>,
	kind: &str,
	id: &str,
	version: &str,
) -> Result<Option<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let sql = Query::select()
		.column((Alias::new("r"), Alias::new("metadata")))
		.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
			Condition::all()
				.add(
					reinhardt::query::SimpleExpr::from(Expr::col((
						Alias::new("r"),
						Alias::new("tenant"),
					)))
					.eq(Expr::col((Alias::new("c"), Alias::new("tenant")))),
				)
				.add(
					reinhardt::query::SimpleExpr::from(Expr::col((
						Alias::new("r"),
						Alias::new("id"),
					)))
					.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
				)
				.add(
					reinhardt::query::SimpleExpr::from(Expr::col((
						Alias::new("r"),
						Alias::new("version"),
					)))
					.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
				),
		)
		.and_where(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::value(tenant)))
		.and_where(Expr::cust(
			"c.node = (SELECT n.name FROM nodes n WHERE n.tenant = c.tenant AND n.is_default)",
		))
		.and_where(
			Expr::col((Alias::new("c"), Alias::new("enabled")))
				.eq(reinhardt::query::Expr::value(true)),
		)
		.and_where(Expr::col((Alias::new("c"), Alias::new("entry_id"))).eq(Expr::value(id)))
		.and_where(
			Expr::col((Alias::new("c"), Alias::new("entry_version"))).eq(Expr::value(version)),
		)
		.limit(1)
		.to_string(PostgresQueryBuilder);
	let entry: Option<serde_json::Value> = crate::database::native::query_scalar(&sql)
		.scalar_optional(authority.connection())
		.await?;
	let entry = entry
		.map(serde_json::from_value::<Box<Entry>>)
		.transpose()?;
	Ok(entry
		.filter(|entry| entry.kind == kind)
		.map(Candidate::Registry))
}

pub(crate) fn encode_cursor(f: &Federation, cursor: &GraphCursor) -> Result<String> {
	let payload = serde_json::to_vec(cursor)?;
	let mut mac = Hmac::<Sha256>::new_from_slice(f.config.api_token.as_bytes())
		.map_err(|_| Error::External("cursor signing unavailable".into()))?;
	mac.update(&payload);
	Ok(format!(
		"{}.{}",
		URL_SAFE_NO_PAD.encode(payload),
		URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
	))
}

pub(crate) fn decode_cursor(f: &Federation, token: &str) -> Result<GraphCursor> {
	let (payload, tag) = token.split_once('.').ok_or(Error::Forbidden)?;
	let payload = URL_SAFE_NO_PAD
		.decode(payload)
		.map_err(|_| Error::Forbidden)?;
	let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| Error::Forbidden)?;
	let mut mac = Hmac::<Sha256>::new_from_slice(f.config.api_token.as_bytes())
		.map_err(|_| Error::External("cursor signing unavailable".into()))?;
	mac.update(&payload);
	mac.verify_slice(&tag).map_err(|_| Error::Forbidden)?;
	serde_json::from_slice(&payload).map_err(|_| Error::Forbidden)
}

async fn aggregate_revision(
	conn: &mut Transaction,
	table: &str,
	column: &str,
	join_column: &str,
	tenant: &str,
) -> Result<(Option<i64>, i64)> {
	crate::database::native::query_as(
		&tenant_resources(table, join_column, tenant)
			.expr(
				reinhardt::query::Func::sum(
					Expr::col((Alias::new("r"), Alias::new(column))).into(),
				)
				.cast_as(Alias::new("int8")),
			)
			.expr(reinhardt::query::Func::count(
				Expr::col((Alias::new("r"), Alias::new(column))).into(),
			))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["sum", "count"])
	.fetch_one(conn)
	.await
}

fn tenant_resources(
	table: &str,
	join_column: &str,
	tenant: &str,
) -> reinhardt::query::SelectStatement {
	Query::select()
		.from_as(Alias::new(table), Alias::new("r"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("authorization_workspaces"),
				Alias::new("a"),
			),
			reinhardt::query::SimpleExpr::from(Expr::col((
				Alias::new("r"),
				Alias::new(join_column),
			)))
			.eq(Expr::col((Alias::new("a"), Alias::new("workspace_id")))),
		)
		.and_where(Expr::col((Alias::new("a"), Alias::new("tenant"))).eq(Expr::value(tenant)))
		.to_owned()
}

pub(crate) async fn graph_generation(
	authority: &mut GraphAuthority<'_>,
	authority_revision: &str,
	options: &GraphOptions,
	source_node: &str,
) -> Result<String> {
	let tenant = authority.tenant().to_owned();
	let catalog: (Option<i64>, i64) = crate::database::native::query_as(
		&Query::select()
			.expr(
				reinhardt::query::Func::sum(Expr::col(Alias::new("revision")).into())
					.cast_as(Alias::new("int8")),
			)
			.expr(reinhardt::query::Func::count(
				Expr::col(Alias::new("revision")).into(),
			))
			.from(Alias::new("authorization_catalog"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.clone())))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["sum", "count"])
	.fetch_one(authority.connection())
	.await?;
	if let Some(workspace) = options.scope_workspace {
		let scope = scoped::revision(authority, options, workspace, source_node).await?;
		return Ok(crate::registry::digest(&json!({
			"authority":authority_revision,"catalog":catalog,"scope":scope,
		})));
	}
	let conn = authority.connection();
	let workspaces =
		aggregate_revision(&mut *conn, "workspaces", "revision", "id", &tenant).await?;
	let tasks =
		aggregate_revision(&mut *conn, "tasks", "revision", "workspace_id", &tenant).await?;
	let runs = aggregate_revision(&mut *conn, "runs", "revision", "workspace_id", &tenant).await?;
	let artifacts: (Option<DateTime<Utc>>, i64) = crate::database::native::query_as(
		&tenant_resources("artifacts", "workspace_id", &tenant)
			.expr(reinhardt::query::Func::max(
				Expr::col((Alias::new("r"), Alias::new("created_at"))).into(),
			))
			.expr(reinhardt::query::Func::count(
				Expr::col((Alias::new("r"), Alias::new("id"))).into(),
			))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["max", "count"])
	.fetch_one(&mut *conn)
	.await?;
	let conversations: (Option<DateTime<Utc>>, i64) = crate::database::native::query_as(
		&tenant_resources("conversations", "workspace_id", &tenant)
			.expr(reinhardt::query::Func::max(
				Expr::col((Alias::new("r"), Alias::new("created_at"))).into(),
			))
			.expr(reinhardt::query::Func::count(
				Expr::col((Alias::new("r"), Alias::new("id"))).into(),
			))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["max", "count"])
	.fetch_one(&mut *conn)
	.await?;
	let events: Option<i64> = crate::database::native::query_scalar(
		&tenant_resources("events", "workspace_id", &tenant)
			.expr(reinhardt::query::Func::max(
				Expr::col((Alias::new("r"), Alias::new("sequence"))).into(),
			))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *conn)
	.await?;
	Ok(crate::registry::digest(&json!({
		"authority":authority_revision,"catalog":catalog,"workspaces":workspaces,
		"tasks":tasks,"runs":runs,"artifacts":artifacts,"conversations":conversations,"events":events,
	})))
}

/// `nodes` is the Node a request comes from and the one the page names as its own.
pub(crate) async fn project_activity(
	authority: &mut GraphAuthority<'_>,
	options: &GraphOptions,
	nodes: &[GraphNode],
	window_end: i64,
	(source_node, node): (&str, &str),
) -> Result<Vec<GraphActivity>> {
	if nodes.is_empty() {
		return Ok(vec![]);
	}
	if let Some(workspace) = options.scope_workspace {
		let end = DateTime::<Utc>::from_timestamp(window_end, 0).ok_or(Error::Forbidden)?;
		return scoped::activity(
			authority,
			options,
			nodes,
			end,
			(source_node, node),
			workspace,
		)
		.await;
	}
	let visible: BTreeSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
	let mut workspaces = Vec::new();
	for workspace in nodes
		.iter()
		.filter_map(|node| node.workspace_id)
		.collect::<BTreeSet<_>>()
	{
		if authority
			.workspace_allowed(workspace, "workspace.events")
			.await?
		{
			workspaces.push(workspace);
		}
	}
	if workspaces.is_empty() {
		return Ok(vec![]);
	}
	let tenant = authority.tenant().to_owned();
	let end = DateTime::<Utc>::from_timestamp(window_end, 0).ok_or(Error::Forbidden)?;
	let start = if options.hours > 0 {
		Some(
			DateTime::<Utc>::from_timestamp(window_end - i64::from(options.hours) * 3600, 0)
				.ok_or(Error::Forbidden)?,
		)
	} else {
		None
	};
	let mut query = Query::select();
	query
		.expr(Expr::cust("e.*"))
		.from_as(Alias::new("events"), Alias::new("e"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("authorization_workspaces"),
				Alias::new("a"),
			),
			reinhardt::query::SimpleExpr::from(Expr::col((
				Alias::new("e"),
				Alias::new("workspace_id"),
			)))
			.eq(Expr::col((Alias::new("a"), Alias::new("workspace_id")))),
		)
		.and_where(
			Expr::col((Alias::new("a"), Alias::new("tenant"))).eq(Expr::value(tenant.clone())),
		)
		.and_where(Expr::col((Alias::new("e"), Alias::new("created_at"))).lte(Expr::value(end)))
		.and_where(
			Expr::col((Alias::new("e"), Alias::new("workspace_id")))
				.is_in(workspaces.iter().copied().map(Expr::value)),
		)
		.cond_where(
			Condition::any()
				.add(Expr::col((Alias::new("e"), Alias::new("kind"))).like("task.%"))
				.add(Expr::col((Alias::new("e"), Alias::new("kind"))).like("artifact.%"))
				.add(Expr::col((Alias::new("e"), Alias::new("kind"))).like("run.%"))
				.add(Expr::col((Alias::new("e"), Alias::new("kind"))).like("conversation.%"))
				.add(Expr::col((Alias::new("e"), Alias::new("kind"))).like("message.%"))
				.add(Expr::col((Alias::new("e"), Alias::new("kind"))).like("workspace.%")),
		)
		.order_by((Alias::new("e"), Alias::new("sequence")), Order::Desc)
		.limit(512);
	if let Some(start) = start {
		query.and_where(
			Expr::col((Alias::new("e"), Alias::new("created_at"))).gte(Expr::value(start)),
		);
	}

	let mut markers = Vec::new();
	let mut before = None;
	let mut scanned = 0;
	loop {
		let mut page = query.clone();
		if let Some(sequence) = before {
			page.and_where(
				Expr::col((Alias::new("e"), Alias::new("sequence")))
					.lt(reinhardt::query::Expr::value(sequence)),
			);
		}
		let sql = page.to_string(PostgresQueryBuilder);
		let events = aidash_server::database::query_as::<Event>(&sql)
			.fetch_all(authority.connection())
			.await?;
		scanned += events.len();
		let exhausted = events.len() < 512;
		before = events.last().map(|event| event.sequence);
		for event in events {
			let Some(reference) = event_reference(&event, node) else {
				continue;
			};
			if !visible.contains(reference.as_str()) || !authority.event_visible(&event).await? {
				continue;
			}
			markers.push(GraphActivity {
				kind: event.kind,
				at: event.created_at,
				reference,
			});
			if markers.len() == 80 {
				break;
			}
		}
		if markers.len() == 80 || exhausted || scanned >= ACTIVITY_SCAN_LIMIT {
			break;
		}
	}
	Ok(markers)
}

pub(crate) async fn candidate_visible(
	f: &Federation,
	authority: &mut GraphAuthority<'_>,
	candidate: &Candidate,
	scoped_workspace: bool,
) -> Result<bool> {
	match candidate {
		Candidate::Run(run) if scoped_workspace => scoped::visible(f, authority, run).await,
		_ => authority.visible(candidate).await,
	}
}

pub(crate) async fn mapping_revision(
	access: &mut Access,
	admitted: &aidash_application::federation::inbound::Admitted,
	subject: &str,
) -> Result<i64> {
	crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("revision"))
			.from(Alias::new("authorization_peer_mappings"))
			.and_where(SimpleExpr::CustomWithExpr(
				"(tenant = ? AND node = ? AND source_node = ? AND source_subject = ? AND enabled)"
					.to_owned(),
				vec![
					Expr::value(admitted.tenant.to_string()).into(),
					Expr::value(admitted.receiver.node.to_string()).into(),
					Expr::value(admitted.sender.to_string()).into(),
					Expr::value(subject).into(),
				],
			))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **access.tx)
	.await
}
