//! Scoped graph projection. A peer connection authenticates the source Node;
//! a mapped Subject or a typed operator grant supplies the receiving authority.
#[path = "graph/catalog.rs"]
mod catalog;
#[path = "graph/scoped.rs"]
mod scoped;

use super::super::{Authorization, access::Access, identity::Actor, policy::identifier};
use crate::{
	Error, Result,
	domain::{Artifact, Conversation, Event, Task, Workspace},
	federation::Federation,
	registry::Entry,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, JoinType, LockType, OnConflict, Order, PostgresQueryBuilder,
	Query,
};
use serde_json::json;
use sha2::Sha256;
use sqlx::{PgConnection, Postgres, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) fn grant_page_size() -> u64 {
	100
}

pub(crate) async fn peers(
	f: Federation,
	actor: Actor,
	origin: Option<crate::dashboard_auth::BrowserOrigin>,
) -> Result<Vec<GraphPeer>> {
	let peers = f.peers().await?;
	match actor {
		Actor::Subject(identity) => {
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let mut visible = Vec::new();
				for peer in peers.into_iter().filter(|peer| peer.enabled) {
					let resource =
						access.resource("node", &peer.node_id, json!({"remote_node":peer.node_id}));
					if access.decide(&resource, "federation.graph.read").await? {
						visible.push(GraphPeer {
							node_id: peer.node_id,
						});
					}
				}
				Ok(visible)
			}
			.await;
			access.finish(result).await
		}
		Actor::Operator if origin.is_some() => Ok(peers
			.into_iter()
			.filter(|peer| peer.enabled)
			.map(|peer| GraphPeer {
				node_id: peer.node_id,
			})
			.collect()),
		Actor::Operator => Err(Error::Forbidden),
	}
}

pub(crate) async fn expand(
	f: Federation,
	actor: Actor,
	origin: Option<crate::dashboard_auth::BrowserOrigin>,
	input: GraphExpandInput,
) -> Result<GraphPage> {
	input.options.validate()?;
	crate::config::validate_node_id(&input.node_id)?;
	match actor {
		Actor::Subject(identity) => {
			if input.options.target_tenant.is_some() {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let resource =
					access.resource("node", &input.node_id, json!({"remote_node":input.node_id}));
				access.require(&resource, "federation.graph.read").await?;
				if let Some(id) = input.options.scope_workspace {
					access.require_workspace(id, "workspace.read").await?;
				}
				source_peer_lease(&mut access.tx, &input.node_id).await?;
				remote_page(
					&f,
					&input,
					GraphViewer::Subject {
						tenant: identity.tenant,
						subject: identity.subject,
					},
				)
				.await
			}
			.await;
			access.finish(result).await
		}
		Actor::Operator => {
			let id = origin.ok_or(Error::Forbidden)?.identity_id;
			if input.options.target_tenant.is_none() {
				return Err(Error::Invalid("target tenant required".into()));
			}
			let mut tx = f.store.pool.begin().await?;
			source_peer_lease(&mut tx, &input.node_id).await?;
			let result = remote_page(&f, &input, GraphViewer::Operator { id }).await;
			if result.is_ok() {
				tx.commit().await?;
			} else {
				tx.rollback().await?;
			}
			result
		}
	}
}

async fn source_peer_lease(tx: &mut Transaction<'static, Postgres>, node: &str) -> Result<()> {
	let enabled: Option<String> = {
		let query_bind_1 = node;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("node_id"))
				.from(Alias::new("peers"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id = ? AND enabled)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **tx)
		.await?
	};
	if enabled.is_none() {
		return Err(Error::Forbidden);
	}
	Ok(())
}

fn valid_remote_page(page: &GraphPage, input: &GraphExpandInput) -> bool {
	aidash_domain::federation::graph::valid_remote_page(page, &input.node_id, &input.options)
}

async fn remote_page(
	f: &Federation,
	input: &GraphExpandInput,
	viewer: GraphViewer,
) -> Result<GraphPage> {
	let request = GraphRequest {
		viewer,
		options: input.options.clone(),
	};
	let response = f
		.peer_response(
			&input.node_id,
			reqwest::Method::POST,
			"/scoped/graph",
			Some(&serde_json::to_value(request)?),
		)
		.await
		.map_err(|_| Error::External("remote graph unavailable".into()))?;
	let status = response.status().as_u16();
	let page: GraphPage = match status {
		200 => crate::response::json(response, 4_194_304)
			.await
			.map_err(|_| Error::External("invalid remote graph response".into()))?,
		401 | 403 => return Err(Error::Forbidden),
		400 => {
			return Err(Error::Invalid(
				"remote graph resource exceeds page limit".into(),
			));
		}
		404 => return Err(Error::NotFound("remote graph feature unavailable".into())),
		409 => return Err(Error::Conflict("remote graph generation changed".into())),
		_ => return Err(Error::External("remote graph unavailable".into())),
	};
	if !valid_remote_page(&page, input) {
		return Err(Error::External("invalid remote graph response".into()));
	}
	Ok(page)
}

pub(crate) async fn project(
	f: Federation,
	headers: HeaderMap,
	input: GraphRequest,
) -> Result<GraphPage> {
	input.options.validate()?;
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let page = project_page(&f, node, input).await?;
	Ok(page)
}

pub(crate) enum GraphAuthority<'a> {
	Subject(&'a mut Access),
	Operator {
		tenant: &'a str,
		tx: &'a mut Transaction<'static, Postgres>,
	},
}

impl GraphAuthority<'_> {
	fn tenant(&self) -> &str {
		match self {
			Self::Subject(access) => &access.identity.tenant,
			Self::Operator { tenant, .. } => tenant,
		}
	}

	fn connection(&mut self) -> &mut PgConnection {
		match self {
			Self::Subject(access) => &mut access.tx,
			Self::Operator { tx, .. } => tx,
		}
	}

	pub(crate) async fn visible(&mut self, candidate: &Candidate) -> Result<bool> {
		let Self::Subject(access) = self else {
			let workspace = match candidate {
				Candidate::Workspace(row) => Some(row.id),
				Candidate::Task(row) => Some(row.workspace_id),
				Candidate::Run(row) => Some(row.workspace_id),
				Candidate::Artifact(row) => Some(row.workspace_id),
				Candidate::Conversation(row) => Some(row.workspace_id),
				Candidate::Registry(_) => None,
			};
			return if let Some(id) = workspace {
				super::super::remote::operator::visible(self.connection(), id).await
			} else {
				Ok(true)
			};
		};
		match candidate {
			Candidate::Registry(entry) => {
				let resource = super::super::catalog::resource(access, entry);
				access.decide(&resource, "registry.read").await
			}
			Candidate::Workspace(workspace) => access.allowed(workspace.id, "workspace.read").await,
			Candidate::Task(task) => Ok(access
				.allowed(task.workspace_id, "workspace.read")
				.await? && access.task_visible(task).await?),
			Candidate::Run(run) => Ok(access.allowed(run.workspace_id, "workspace.read").await?
				&& access.run_visible(run).await?),
			Candidate::Artifact(artifact) => {
				Ok(access
					.allowed(artifact.workspace_id, "workspace.read")
					.await? && access.artifact_visible(artifact).await?)
			}
			Candidate::Conversation(conversation) => {
				if !access
					.allowed(conversation.workspace_id, "workspace.read")
					.await?
				{
					return Ok(false);
				}
				let resource = access.conversation_resource(conversation).await?;
				access.decide(&resource, "conversation.read").await
			}
		}
	}

	async fn event_visible(&mut self, event: &Event) -> Result<bool> {
		let Self::Subject(access) = self else {
			return super::super::remote::operator::event_visible(self.connection(), event).await;
		};
		// project_activity checks workspace.events once per workspace. Retain
		// the event-specific resource and provenance checks without repeating it.
		access.event_visible(event).await
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
	let entry: Option<serde_json::Value> = sqlx::query_scalar(&sql)
		.fetch_optional(authority.connection())
		.await?;
	let entry = entry.map(serde_json::from_value::<Entry>).transpose()?;
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
	conn: &mut PgConnection,
	table: &str,
	column: &str,
	join_column: &str,
	tenant: &str,
) -> Result<(Option<i64>, i64)> {
	Ok(sqlx::query_as(
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
	.fetch_one(conn)
	.await?)
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
	f: &Federation,
	authority: &mut GraphAuthority<'_>,
	authority_revision: &str,
	options: &GraphOptions,
	source_node: &str,
) -> Result<String> {
	let tenant = authority.tenant().to_owned();
	let catalog: (Option<i64>, i64) = sqlx::query_as(
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
	.fetch_one(authority.connection())
	.await?;
	if let Some(workspace) = options.scope_workspace {
		let scope = scoped::revision(
			authority,
			options,
			workspace,
			source_node,
			&f.config.node_id,
		)
		.await?;
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
	let artifacts: (Option<DateTime<Utc>>, i64) = sqlx::query_as(
		&tenant_resources("artifacts", "workspace_id", &tenant)
			.expr(reinhardt::query::Func::max(
				Expr::col((Alias::new("r"), Alias::new("created_at"))).into(),
			))
			.expr(reinhardt::query::Func::count(
				Expr::col((Alias::new("r"), Alias::new("id"))).into(),
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *conn)
	.await?;
	let conversations: (Option<DateTime<Utc>>, i64) = sqlx::query_as(
		&tenant_resources("conversations", "workspace_id", &tenant)
			.expr(reinhardt::query::Func::max(
				Expr::col((Alias::new("r"), Alias::new("created_at"))).into(),
			))
			.expr(reinhardt::query::Func::count(
				Expr::col((Alias::new("r"), Alias::new("id"))).into(),
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *conn)
	.await?;
	let events: Option<i64> = sqlx::query_scalar(
		&tenant_resources("events", "workspace_id", &tenant)
			.expr(reinhardt::query::Func::max(
				Expr::col((Alias::new("r"), Alias::new("sequence"))).into(),
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *conn)
	.await?;
	Ok(crate::registry::digest(&json!({
		"authority":authority_revision,"catalog":catalog,"workspaces":workspaces,
		"tasks":tasks,"runs":runs,"artifacts":artifacts,"conversations":conversations,"events":events,
	})))
}

pub(crate) async fn project_activity(
	f: &Federation,
	authority: &mut GraphAuthority<'_>,
	options: &GraphOptions,
	nodes: &[GraphNode],
	window_end: i64,
	source_node: &str,
) -> Result<Vec<GraphActivity>> {
	if nodes.is_empty() {
		return Ok(vec![]);
	}
	if let Some(workspace) = options.scope_workspace {
		let end = DateTime::<Utc>::from_timestamp(window_end, 0).ok_or(Error::Forbidden)?;
		return scoped::activity(f, authority, options, nodes, end, source_node, workspace).await;
	}
	let visible: BTreeSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
	let workspaces: Vec<Uuid> = nodes
		.iter()
		.filter_map(|node| node.workspace_id)
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect();
	let workspaces = if let GraphAuthority::Subject(access) = authority {
		let mut allowed = Vec::new();
		for workspace in workspaces {
			if access.allowed(workspace, "workspace.events").await? {
				allowed.push(workspace);
			}
		}
		allowed
	} else {
		workspaces
	};
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
		.and_where(
			Expr::col((Alias::new("e"), Alias::new("node_id")))
				.eq(Expr::value(f.config.node_id.clone())),
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
			let Some(reference) = event_reference(&event, &f.config.node_id) else {
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

async fn project_in(
	f: &Federation,
	source_node: &str,
	viewer: &GraphViewer,
	options: &GraphOptions,
	authority: &mut GraphAuthority<'_>,
	authority_revision: &str,
) -> Result<GraphPage> {
	aidash_application::federation::graph::project(
		&mut crate::bootstrap::graph_projection_scope(
			f,
			authority,
			source_node,
			authority_revision,
		),
		source_node,
		viewer,
		options,
	)
	.await
	.map_err(Into::into)
}

async fn project_page(f: &Federation, node: &str, input: GraphRequest) -> Result<GraphPage> {
	match &input.viewer {
		GraphViewer::Subject { tenant, subject } => {
			if input.options.target_tenant.is_some() {
				return Err(Error::Forbidden);
			}
			let mut access = super::access(f, node, tenant, subject).await?;
			let result = async {
				let resource = access.resource("node", &f.config.node_id, json!({}));
				access.require(&resource, "federation.graph.read").await?;
				let mapping_revision: i64 = { let query_bind_1 = node; let query_bind_2 = tenant; let query_bind_3 = subject; sqlx::query_scalar(&Query::select()
						.column(Alias::new("revision"))
						.from(Alias::new("authorization_peer_mappings"))
						.and_where(SimpleExpr::CustomWithExpr("(source_node = ? AND source_tenant = ? AND source_subject = ? AND enabled)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
						.to_string(PostgresQueryBuilder))
				.fetch_one(&mut **access.tx)
				.await? };
				let revision = format!(
					"{}:{}:{}",
					access.snapshot.revision, access.identity.credential_id, mapping_revision
				);
				project_in(
					f,
					node,
					&input.viewer,
					&input.options,
					&mut GraphAuthority::Subject(&mut access),
					&revision,
				)
				.await
			}
			.await;
			access.finish(result).await
		}
		GraphViewer::Operator { id } => {
			let tenant = input
				.options
				.target_tenant
				.as_deref()
				.ok_or(Error::Forbidden)?;
			let (mut tx, grant) = operator_grant(f, node, *id, tenant).await?;
			let revision = format!("{}:{}", grant.source_operator, grant.revision);
			let result = project_in(
				f,
				node,
				&input.viewer,
				&input.options,
				&mut GraphAuthority::Operator {
					tenant,
					tx: &mut tx,
				},
				&revision,
			)
			.await;
			if result.is_ok() {
				tx.commit().await?;
			} else {
				tx.rollback().await?;
			}
			result
		}
	}
}

pub(crate) async fn list_grants(
	f: Federation,
	tenant: String,
	QueryParams(page): QueryParams<GrantPage>,
) -> Result<Vec<GraphOperatorGrant>> {
	identifier(&tenant)?;
	if !(1..=200).contains(&page.limit) {
		return Err(Error::Invalid("invalid grant page".into()));
	}
	let grants = {
		let query_bind_1 = tenant;
		sqlx::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("authorization_graph_operator_grants"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(query_bind_1.to_owned())))
				.order_by(Alias::new("source_node"), Order::Asc)
				.order_by(Alias::new("source_operator"), Order::Asc)
				.limit(page.limit)
				.offset(page.offset)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&f.store.pool)
		.await?
	};
	Ok(grants)
}

pub(crate) async fn set_grant(
	f: Federation,
	tenant: String,
	input: GraphOperatorGrantInput,
) -> Result<GraphOperatorGrant> {
	identifier(&tenant)?;
	crate::config::validate_node_id(&input.source_node)?;
	if input.source_node == f.config.node_id || !(0..i64::MAX).contains(&input.expected_revision) {
		return Err(Error::Invalid(
			"invalid graph operator grant or revision".into(),
		));
	}
	if input.enabled {
		f.peer(&input.source_node).await?;
	} else if input.expected_revision == 0 {
		return Err(Error::Invalid(
			"revocation requires an existing grant".into(),
		));
	}
	let mut tx = if input.enabled {
		f.store.pool.begin().await?
	} else {
		f.store.control_pool.begin().await?
	};
	if !input.enabled {
		crate::transactions::authority::control(&mut tx).await?;
	}
	Authorization::load(&mut tx, &tenant).await?;
	let row: Option<GraphOperatorGrant> = if input.expected_revision == 0 {
		{
			let query_bind_1 = &input.source_node;
			let query_bind_2 = input.source_operator;
			let query_bind_3 = &tenant;
			let query_bind_4 = input.enabled;
			sqlx::query_as(
				&Query::insert()
					.into_table(Alias::new("authorization_graph_operator_grants"))
					.columns([
						Alias::new("source_node"),
						Alias::new("source_operator"),
						Alias::new("tenant"),
						Alias::new("enabled"),
						Alias::new("revision"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(Expr::value(query_bind_1.to_owned()))
							.expr(Expr::value(query_bind_2.to_owned()))
							.expr(Expr::value(query_bind_3.to_owned()))
							.expr(Expr::value(query_bind_4.to_owned()))
							.expr(Expr::value(1_i64))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns(["source_node", "source_operator", "tenant"])
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?
		}
	} else {
		{
			let query_bind_1 = &input.source_node;
			let query_bind_2 = input.source_operator;
			let query_bind_3 = &tenant;
			let query_bind_4 = input.enabled;
			let query_bind_5 = input.expected_revision;
			sqlx::query_as(
				&Query::update()
					.table(Alias::new("authorization_graph_operator_grants"))
					.value_expr(
						Alias::new("enabled"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						),
					)
					.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
					.value_expr(Alias::new("updated_at"), Expr::cust("clock_timestamp()"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(source_node = ? AND source_operator = ? AND tenant = ? AND revision = ?)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
							Expr::value(query_bind_5.to_owned()).into(),
						],
					))
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?
		}
	};
	let grant =
		row.ok_or_else(|| Error::Conflict("graph operator grant revision changed".into()))?;
	tx.commit().await?;
	Ok(grant)
}

pub(crate) async fn operator_grant(
	f: &Federation,
	node: &str,
	operator: Uuid,
	tenant: &str,
) -> Result<(
	sqlx::Transaction<'static, sqlx::Postgres>,
	GraphOperatorGrant,
)> {
	identifier(tenant)?;
	let mut tx = f.store.pool.begin().await?;
	Authorization::load(&mut tx, tenant).await?;
	let grant: GraphOperatorGrant = {
		let query_bind_1 = node;
		let query_bind_2 = operator;
		let query_bind_3 = tenant;
		sqlx::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("authorization_graph_operator_grants"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(source_node = ? AND source_operator = ? AND tenant = ? AND enabled)"
						.to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut *tx)
		.await?
	}
	.ok_or(Error::Forbidden)?;
	let enabled: Option<String> = {
		let query_bind_1 = node;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("node_id"))
				.from(Alias::new("peers"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id = ? AND enabled)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut *tx)
		.await?
	};
	if enabled.is_none() {
		return Err(Error::Forbidden);
	}
	Ok((tx, grant))
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use http::HeaderMap;

#[derive(Clone)]
pub struct GraphManagement {
	pub(crate) runtime: Federation,
}
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> GraphManagement {
	tracing::trace!(service = "GraphManagement", "creating injectable service");
	GraphManagement { runtime }
}

use reinhardt::injectable;

use reinhardt::Query as QueryParams;

pub use crate::apps::identity::serializers::peer_graph::{
	GraphActivity, GraphExpandInput, GraphNode, GraphOperatorGrant, GraphOperatorGrantInput,
	GraphOptions, GraphPage, GraphPeer, GraphRequest,
};

pub(crate) use crate::apps::identity::serializers::peer_graph::{
	GrantPage, GraphCursor, GraphViewer,
};

#[cfg(test)]
#[path = "../../tests/services_peer_graph_tests.rs"]
mod tests;

use crate::domain::RunMetadata;

use reinhardt::query::SimpleExpr;

use aidash_domain::federation::graph::{CANDIDATE_BATCH, Candidate, event_reference, kind_allowed};

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

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use crate::apps::identity::serializers::peer_graph::GraphEdge;
#[cfg(test)]
use aidash_domain::federation::graph::entity_key;
