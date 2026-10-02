//! Scoped graph projection. A peer connection authenticates the source Node;
//! a mapped Subject or a typed operator grant supplies the receiving authority.
mod catalog;
mod scoped;

use super::super::{Authorization, access::Access, identity::Actor, policy::identifier};
use crate::{
	Error, Result,
	domain::{Artifact, Conversation, Event, RunMetadata, Task, Workspace},
	federation::Federation,
	registry::Entry,
};
use axum::{
	Extension, Json,
	extract::{Path, Query as QueryParams, State},
	http::HeaderMap,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sea_orm::sea_query::{
	Alias, Asterisk, Condition, Expr, JoinType, LockType, OnConflict, Order, PostgresQueryBuilder,
	Query,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use sqlx::{PgConnection, Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct GraphOperatorGrant {
	pub source_node: String,
	pub source_operator: Uuid,
	pub tenant: String,
	pub enabled: bool,
	pub revision: i64,
	pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphOperatorGrantInput {
	pub source_node: String,
	pub source_operator: Uuid,
	pub enabled: bool,
	pub expected_revision: i64,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct GrantPage {
	#[serde(default)]
	offset: u64,
	#[serde(default = "grant_page_size")]
	limit: u64,
}

fn grant_page_size() -> u64 {
	100
}

pub fn control_routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new().routes(routes!(list_grants, set_grant))
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphOptions {
	pub scope_workspace: Option<Uuid>,
	#[serde(default = "direct_depth")]
	pub depth: u8,
	pub mode: String,
	pub kinds: Vec<String>,
	pub relations: Vec<String>,
	pub hours: u16,
	pub limit: u16,
	pub cursor: Option<String>,
	pub target_tenant: Option<String>,
}

fn direct_depth() -> u8 {
	1
}

impl GraphOptions {
	fn validate(&self) -> Result<()> {
		if !matches!(
			self.mode.as_str(),
			"mesh" | "collaboration" | "knowledge" | "execution" | "topology"
		) || self.depth != direct_depth()
			|| self.limit < 2
			|| self.limit > 80
			|| self.hours > 720
			|| self.kinds.len() > 12
			|| self.relations.len() > 16
			|| self
				.cursor
				.as_ref()
				.is_some_and(|cursor| cursor.len() > 4096)
			|| self.kinds.iter().any(|kind| {
				!matches!(
					kind.as_str(),
					"workspace"
						| "goal" | "task" | "run"
						| "artifact" | "conversation"
						| "agent" | "cluster"
						| "tool" | "model" | "skill"
				)
			}) || self.relations.iter().any(|relation| {
			!matches!(
				relation.as_str(),
				"contains"
					| "goal" | "depends"
					| "produces" | "executes"
					| "tool" | "model"
					| "skill" | "member"
					| "hosts" | "coordinates"
					| "participates"
			)
		}) {
			return Err(Error::Invalid("invalid graph bounds".into()));
		}
		if let Some(tenant) = &self.target_tenant {
			identifier(tenant)?;
		}
		Ok(())
	}
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphExpandInput {
	pub node_id: String,
	#[serde(flatten)]
	pub options: GraphOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum GraphViewer {
	Subject { tenant: String, subject: String },
	Operator { id: Uuid },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphRequest {
	viewer: GraphViewer,
	#[serde(flatten)]
	options: GraphOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GraphNode {
	pub id: String,
	pub node_id: String,
	pub kind: String,
	pub name: BTreeMap<String, String>,
	pub resource_id: Option<String>,
	pub version: Option<String>,
	pub workspace_id: Option<Uuid>,
	pub status: Option<String>,
	pub goal_body: Option<String>,
	pub at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GraphEdge {
	pub source: String,
	pub target: String,
	pub relation: String,
	pub layer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GraphPage {
	pub node_id: String,
	pub generation: String,
	pub checked_at: DateTime<Utc>,
	pub nodes: Vec<GraphNode>,
	pub edges: Vec<GraphEdge>,
	pub activity: Vec<GraphActivity>,
	pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GraphActivity {
	pub kind: String,
	pub at: DateTime<Utc>,
	pub reference: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GraphPeer {
	pub node_id: String,
}

pub fn api_routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new().routes(routes!(peers, expand))
}

#[utoipa::path(get,path="/federation/graph/peers",operation_id="federated_graph_peers",responses((status=200,body=[GraphPeer])),security(("bearer_auth"=[])))]
async fn peers(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	origin: Option<Extension<crate::dashboard_auth::BrowserOrigin>>,
) -> Result<Json<Vec<GraphPeer>>> {
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
				Ok(Json(visible))
			}
			.await;
			access.finish(result).await
		}
		Actor::Operator if origin.is_some() => Ok(Json(
			peers
				.into_iter()
				.filter(|peer| peer.enabled)
				.map(|peer| GraphPeer {
					node_id: peer.node_id,
				})
				.collect(),
		)),
		Actor::Operator => Err(Error::Forbidden),
	}
}

#[utoipa::path(post,path="/federation/graph",operation_id="federated_graph_expand",request_body=GraphExpandInput,responses((status=200,body=GraphPage)),security(("bearer_auth"=[])))]
async fn expand(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	origin: Option<Extension<crate::dashboard_auth::BrowserOrigin>>,
	Json(input): Json<GraphExpandInput>,
) -> Result<Json<GraphPage>> {
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
			let id = origin.ok_or(Error::Forbidden)?.0.identity_id;
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
	let enabled: Option<String> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("node_id"))
			.from(Alias::new("peers"))
			.and_where(Expr::cust("node_id = $1 AND enabled"))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(node)
	.fetch_optional(&mut **tx)
	.await?;
	if enabled.is_none() {
		return Err(Error::Forbidden);
	}
	Ok(())
}

fn valid_remote_page(page: &GraphPage, input: &GraphExpandInput) -> bool {
	if page.node_id != input.node_id
		|| page.nodes.len() > 80
		|| page.edges.len() > 600
		|| page.activity.len() > 80
		|| page
			.next_cursor
			.as_ref()
			.is_some_and(|cursor| cursor.len() > 4096)
		|| !page
			.generation
			.strip_prefix("sha256:")
			.is_some_and(|digest| {
				digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
			}) {
		return false;
	}
	let mut ids = BTreeSet::new();
	for node in &page.nodes {
		let registry = matches!(
			node.kind.as_str(),
			"agent" | "cluster" | "tool" | "model" | "skill"
		);
		if node.node_id != input.node_id
			|| !kind_allowed(&node.kind, &input.options)
			|| node.resource_id.as_deref().is_none_or(str::is_empty)
			|| node.goal_body.is_some() && node.kind != "goal"
			|| registry != node.version.is_some()
		{
			return false;
		}
		let resource = node.resource_id.as_deref().unwrap_or_default();
		let expected = match &node.version {
			Some(version) => entity_key(&input.node_id, &node.kind, resource, version),
			None => resource_key(&input.node_id, &node.kind, resource),
		};
		if node.id != expected || !ids.insert(node.id.as_str()) {
			return false;
		}
	}
	page.activity
		.iter()
		.all(|item| ids.contains(item.reference.as_str()) && item.kind.len() <= 100)
		&& page.edges.iter().all(|edge| {
			ids.contains(edge.source.as_str())
				&& ids.contains(edge.target.as_str())
				&& input
					.options
					.relations
					.iter()
					.any(|relation| relation == &edge.relation)
				&& matches!(
					edge.layer.as_str(),
					"configuration" | "activity" | "federation"
				)
		})
}

async fn remote_page(
	f: &Federation,
	input: &GraphExpandInput,
	viewer: GraphViewer,
) -> Result<Json<GraphPage>> {
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
	Ok(Json(page))
}

pub async fn project(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<GraphRequest>,
) -> Result<Json<GraphPage>> {
	input.options.validate()?;
	let node = crate::api::peer_node(&headers)?;
	let page = project_page(&f, node, input).await?;
	Ok(Json(page))
}

enum GraphAuthority<'a> {
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

	async fn visible(&mut self, candidate: &Candidate) -> Result<bool> {
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

#[derive(Clone)]
enum Candidate {
	Registry(Entry),
	Workspace(Workspace),
	Task(Task),
	Run(RunMetadata),
	Artifact(Artifact),
	Conversation(Conversation),
}

impl Candidate {
	fn kind(&self) -> &str {
		match self {
			Self::Registry(entry) => &entry.kind,
			Self::Workspace(_) => "workspace",
			Self::Task(_) => "task",
			Self::Run(_) => "run",
			Self::Artifact(_) => "artifact",
			Self::Conversation(_) => "conversation",
		}
	}

	fn nodes(&self, node_id: &str, options: &GraphOptions) -> Vec<GraphNode> {
		let mut nodes = Vec::new();
		let name = |text: &str| BTreeMap::from([("en".to_owned(), text.to_owned())]);
		match self {
			Self::Registry(entry) => nodes.push(GraphNode {
				id: entity_key(node_id, &entry.kind, &entry.id, &entry.version),
				node_id: node_id.to_owned(),
				kind: entry.kind.clone(),
				name: entry.name.clone(),
				resource_id: Some(entry.id.clone()),
				version: Some(entry.version.clone()),
				workspace_id: None,
				status: None,
				goal_body: None,
				at: None,
			}),
			Self::Workspace(workspace) => {
				if kind_allowed("workspace", options) {
					nodes.push(GraphNode {
						id: resource_key(node_id, "workspace", workspace.id),
						node_id: node_id.to_owned(),
						kind: "workspace".into(),
						name: name(&workspace.title),
						resource_id: Some(workspace.id.to_string()),
						version: None,
						workspace_id: Some(workspace.id),
						status: None,
						goal_body: None,
						at: Some(workspace.created_at),
					});
				}
				if !workspace.goal.trim().is_empty() && kind_allowed("goal", options) {
					let label = workspace
						.goal
						.lines()
						.find(|line| !line.trim().is_empty())
						.unwrap_or("Goal");
					nodes.push(GraphNode {
						id: resource_key(node_id, "goal", workspace.id),
						node_id: node_id.to_owned(),
						kind: "goal".into(),
						name: name(&label.chars().take(100).collect::<String>()),
						resource_id: Some(workspace.id.to_string()),
						version: None,
						workspace_id: Some(workspace.id),
						status: None,
						goal_body: Some(workspace.goal.clone()),
						at: Some(workspace.created_at),
					});
				}
			}
			Self::Task(task) => nodes.push(GraphNode {
				id: resource_key(node_id, "task", task.id),
				node_id: node_id.to_owned(),
				kind: "task".into(),
				name: name(&task.title),
				resource_id: Some(task.id.to_string()),
				version: None,
				workspace_id: Some(task.workspace_id),
				status: Some(task.status.to_string()),
				goal_body: None,
				at: Some(task.created_at),
			}),
			Self::Run(run) => nodes.push(GraphNode {
				id: resource_key(node_id, "run", run.id),
				node_id: node_id.to_owned(),
				kind: "run".into(),
				name: name(&format!("Run {}", run.id.simple())),
				resource_id: Some(run.id.to_string()),
				version: None,
				workspace_id: Some(run.workspace_id),
				status: Some(if run.control == crate::domain::RunControl::Paused {
					"PAUSED".into()
				} else {
					run.phase().to_string()
				}),
				goal_body: None,
				at: Some(run.updated_at),
			}),
			Self::Artifact(artifact) => nodes.push(GraphNode {
				id: resource_key(node_id, "artifact", artifact.id),
				node_id: node_id.to_owned(),
				kind: "artifact".into(),
				name: name(&artifact.name),
				resource_id: Some(artifact.id.to_string()),
				version: None,
				workspace_id: Some(artifact.workspace_id),
				status: Some(artifact.kind.clone()),
				goal_body: None,
				at: Some(artifact.created_at),
			}),
			Self::Conversation(conversation) => nodes.push(GraphNode {
				id: resource_key(node_id, "conversation", conversation.id),
				node_id: node_id.to_owned(),
				kind: "conversation".into(),
				name: name("Conversation"),
				resource_id: Some(conversation.id.to_string()),
				version: None,
				workspace_id: Some(conversation.workspace_id),
				status: None,
				goal_body: None,
				at: Some(conversation.created_at),
			}),
		}
		nodes
	}
}

fn resource_key(node: &str, kind: &str, id: impl ToString) -> String {
	json!(["resource", node, kind, id.to_string()]).to_string()
}

fn entity_key(node: &str, kind: &str, id: &str, version: &str) -> String {
	json!(["entity", node, kind, id, version]).to_string()
}

fn kind_allowed(kind: &str, options: &GraphOptions) -> bool {
	if !options.kinds.iter().any(|allowed| allowed == kind) {
		return false;
	}
	match options.mode.as_str() {
		"topology" => matches!(kind, "agent" | "cluster" | "tool"),
		"execution" => matches!(kind, "goal" | "task" | "run" | "agent" | "artifact"),
		"collaboration" => matches!(
			kind,
			"workspace" | "goal" | "conversation" | "task" | "agent" | "cluster" | "artifact"
		),
		"knowledge" => kind != "run",
		_ => true,
	}
}

const CANDIDATE_BATCH: u64 = 64;
const CANDIDATE_KINDS: u8 = 6;
const ACTIVITY_SCAN_LIMIT: usize = 4096;

async fn candidates(
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
		.column((Alias::new("r"), Asterisk))
		.from_as(Alias::new(table), Alias::new("r"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("authorization_workspaces"),
			Alias::new("a"),
			Expr::col((Alias::new("r"), Alias::new(join_column)))
				.eq(Expr::col((Alias::new("a"), Alias::new("workspace_id")))),
		)
		.and_where(Expr::col((Alias::new("a"), Alias::new("tenant"))).eq(Expr::cust("$1")))
		.order_by((Alias::new("r"), Alias::new("id")), Order::Asc)
		.limit(CANDIDATE_BATCH)
		.offset(offset)
		.to_string(PostgresQueryBuilder);
	match kind {
		0 => Ok(sqlx::query_as::<_, Workspace>(&sql)
			.bind(&tenant)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Workspace)
			.collect()),
		1 => Ok(sqlx::query_as::<_, Task>(&sql)
			.bind(&tenant)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Task)
			.collect()),
		2 => Ok(sqlx::query_as::<_, Artifact>(&sql)
			.bind(&tenant)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Artifact)
			.collect()),
		3 => Ok(sqlx::query_as::<_, RunMetadata>(&sql)
			.bind(&tenant)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Run)
			.collect()),
		4 => Ok(sqlx::query_as::<_, Conversation>(&sql)
			.bind(&tenant)
			.fetch_all(conn)
			.await?
			.into_iter()
			.map(Candidate::Conversation)
			.collect()),
		_ => unreachable!(),
	}
}

async fn linked_workspace(
	authority: &mut GraphAuthority<'_>,
	id: Uuid,
) -> Result<Option<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let sql = tenant_resources("workspaces", "id")
		.column((Alias::new("r"), Asterisk))
		.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).eq(Expr::cust("$2")))
		.limit(1)
		.to_string(PostgresQueryBuilder);
	Ok(sqlx::query_as::<_, Workspace>(&sql)
		.bind(tenant)
		.bind(id)
		.fetch_optional(authority.connection())
		.await?
		.map(Candidate::Workspace))
}

async fn linked_task(authority: &mut GraphAuthority<'_>, id: Uuid) -> Result<Option<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let sql = tenant_resources("tasks", "workspace_id")
		.column((Alias::new("r"), Asterisk))
		.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).eq(Expr::cust("$2")))
		.limit(1)
		.to_string(PostgresQueryBuilder);
	Ok(sqlx::query_as::<_, Task>(&sql)
		.bind(tenant)
		.bind(id)
		.fetch_optional(authority.connection())
		.await?
		.map(Candidate::Task))
}

async fn linked_registry(
	authority: &mut GraphAuthority<'_>,
	kind: &str,
	id: &str,
	version: &str,
) -> Result<Option<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let sql = Query::select()
		.column((Alias::new("r"), Alias::new("metadata")))
		.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("registry"),
			Alias::new("r"),
			Condition::all()
				.add(
					Expr::col((Alias::new("r"), Alias::new("id")))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
				)
				.add(
					Expr::col((Alias::new("r"), Alias::new("version")))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
				),
		)
		.and_where(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::cust("$1")))
		.and_where(Expr::col((Alias::new("c"), Alias::new("enabled"))).eq(true))
		.and_where(Expr::col((Alias::new("c"), Alias::new("entry_id"))).eq(Expr::cust("$2")))
		.and_where(Expr::col((Alias::new("c"), Alias::new("entry_version"))).eq(Expr::cust("$3")))
		.limit(1)
		.to_string(PostgresQueryBuilder);
	let entry: Option<serde_json::Value> = sqlx::query_scalar(&sql)
		.bind(tenant)
		.bind(id)
		.bind(version)
		.fetch_optional(authority.connection())
		.await?;
	let entry = entry.map(serde_json::from_value::<Entry>).transpose()?;
	Ok(entry
		.filter(|entry| entry.kind == kind)
		.map(Candidate::Registry))
}

fn relation_allowed(relation: &str, options: &GraphOptions) -> bool {
	options.relations.iter().any(|allowed| allowed == relation)
}

async fn linked_candidates(
	authority: &mut GraphAuthority<'_>,
	candidate: &Candidate,
	options: &GraphOptions,
	node: &str,
	visible: &[GraphNode],
) -> Result<Vec<Candidate>> {
	let mut linked = Vec::new();
	let mut seen: BTreeSet<String> = visible.iter().map(|item| item.id.clone()).collect();
	for item in candidate.nodes(node, options) {
		seen.insert(item.id);
	}
	let mut workspace_ids = Vec::new();
	let mut task_ids = Vec::new();
	let mut registry = Vec::<(String, String, String)>::new();
	match candidate {
		Candidate::Workspace(_) => {}
		Candidate::Task(task) if options.scope_workspace.is_none() => {
			if (kind_allowed("workspace", options) || kind_allowed("goal", options))
				&& relation_allowed("contains", options)
			{
				workspace_ids.push(task.workspace_id);
			}
			if kind_allowed("task", options) {
				if relation_allowed("contains", options) {
					task_ids.extend(task.parent_id);
				}
				if relation_allowed("depends", options) {
					task_ids.extend(task.dependencies.iter().copied());
				}
			}
		}
		Candidate::Artifact(artifact) if options.scope_workspace.is_none() => {
			if kind_allowed("task", options) && relation_allowed("produces", options) {
				task_ids.push(artifact.task_id);
			}
		}
		Candidate::Run(run) if relation_allowed("executes", options) => {
			if options.scope_workspace.is_none() && kind_allowed("task", options) {
				task_ids.push(run.task_id);
			}
			if kind_allowed("agent", options) {
				registry.push((
					"agent".into(),
					run.agent_id.clone(),
					run.agent_version.clone(),
				));
			}
		}
		Candidate::Conversation(conversation) if options.scope_workspace.is_none() => {
			if kind_allowed("workspace", options) && relation_allowed("contains", options) {
				workspace_ids.push(conversation.workspace_id);
			}
			if relation_allowed("participates", options)
				&& kind_allowed(&conversation.target_kind, options)
				&& matches!(conversation.target_kind.as_str(), "agent" | "cluster")
				&& let Some((id, version)) = conversation.target.rsplit_once('@')
			{
				registry.push((conversation.target_kind.clone(), id.into(), version.into()));
			}
		}
		Candidate::Registry(entry) => {
			if entry.kind == "cluster"
				&& relation_allowed("coordinates", options)
				&& kind_allowed("agent", options)
				&& let Some((id, version)) = registry_ref(&entry.config["coordinator"])
			{
				registry.push(("agent".into(), id.into(), version.into()));
			}
			if entry.kind == "agent" {
				for (kind, field, relation) in [
					("tool", "tools", "tool"),
					("model", "model", "model"),
					("skill", "skills", "skill"),
					("cluster", "cluster", "member"),
				] {
					if !kind_allowed(kind, options) || !relation_allowed(relation, options) {
						continue;
					}
					let value = &entry.config[field];
					let refs = value
						.as_array()
						.map(|items| items.iter().collect::<Vec<_>>())
						.unwrap_or_else(|| vec![value]);
					for item in refs {
						if let Some((id, version)) = registry_ref(item) {
							registry.push((kind.into(), id.into(), version.into()));
						}
					}
				}
			}
		}
		_ => {}
	}
	task_ids.truncate(options.limit as usize);
	registry.truncate(options.limit as usize);
	for id in workspace_ids {
		if let Some(item) = linked_workspace(authority, id).await? {
			let new_nodes = item.nodes(node, options);
			if new_nodes.iter().any(|node| !seen.contains(&node.id))
				&& authority.visible(&item).await?
			{
				seen.extend(new_nodes.into_iter().map(|node| node.id));
				linked.push(item);
			}
		}
	}
	for id in task_ids {
		if seen.contains(&resource_key(node, "task", id)) {
			continue;
		}
		if let Some(item) = linked_task(authority, id).await?
			&& authority.visible(&item).await?
		{
			seen.extend(item.nodes(node, options).into_iter().map(|node| node.id));
			linked.push(item);
		}
	}
	for (kind, id, version) in registry {
		if seen.contains(&entity_key(node, &kind, &id, &version)) {
			continue;
		}
		if let Some(item) = linked_registry(authority, &kind, &id, &version).await?
			&& authority.visible(&item).await?
		{
			seen.extend(item.nodes(node, options).into_iter().map(|node| node.id));
			linked.push(item);
		}
	}
	Ok(linked)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphCursor {
	kind: u8,
	offset: u64,
	generation: String,
	binding: String,
	window_end: i64,
	expires_at: i64,
}

fn cursor_binding(node: &str, viewer: &GraphViewer, options: &GraphOptions) -> Result<String> {
	let mut scope = options.clone();
	scope.cursor = None;
	Ok(crate::registry::digest(
		&json!({"node":node,"viewer":viewer,"scope":scope}),
	))
}

fn encode_cursor(f: &Federation, cursor: &GraphCursor) -> Result<String> {
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

fn decode_cursor(
	f: &Federation,
	token: &str,
	binding: &str,
	generation: &str,
) -> Result<GraphCursor> {
	let (payload, tag) = token.split_once('.').ok_or(Error::Forbidden)?;
	let payload = URL_SAFE_NO_PAD
		.decode(payload)
		.map_err(|_| Error::Forbidden)?;
	let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| Error::Forbidden)?;
	let mut mac = Hmac::<Sha256>::new_from_slice(f.config.api_token.as_bytes())
		.map_err(|_| Error::External("cursor signing unavailable".into()))?;
	mac.update(&payload);
	mac.verify_slice(&tag).map_err(|_| Error::Forbidden)?;
	let cursor: GraphCursor = serde_json::from_slice(&payload).map_err(|_| Error::Forbidden)?;
	if cursor.binding != binding
		|| cursor.kind >= CANDIDATE_KINDS
		|| cursor.window_end > Utc::now().timestamp() + 60
		|| cursor.expires_at - cursor.window_end != 300
	{
		return Err(Error::Forbidden);
	}
	if cursor.expires_at <= Utc::now().timestamp() || cursor.generation != generation {
		return Err(Error::Conflict("graph projection changed".into()));
	}
	Ok(cursor)
}

async fn aggregate_revision(
	conn: &mut PgConnection,
	table: &str,
	column: &str,
	join_column: &str,
	tenant: &str,
) -> Result<(Option<i64>, i64)> {
	Ok(sqlx::query_as(
		&tenant_resources(table, join_column)
			.expr(
				Expr::col((Alias::new("r"), Alias::new(column)))
					.sum()
					.cast_as(Alias::new("bigint")),
			)
			.expr(Expr::col((Alias::new("r"), Alias::new(column))).count())
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.fetch_one(conn)
	.await?)
}

fn tenant_resources(table: &str, join_column: &str) -> sea_orm::sea_query::SelectStatement {
	Query::select()
		.from_as(Alias::new(table), Alias::new("r"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("authorization_workspaces"),
			Alias::new("a"),
			Expr::col((Alias::new("r"), Alias::new(join_column)))
				.eq(Expr::col((Alias::new("a"), Alias::new("workspace_id")))),
		)
		.and_where(Expr::col((Alias::new("a"), Alias::new("tenant"))).eq(Expr::cust("$1")))
		.to_owned()
}

async fn graph_generation(
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
				Expr::col(Alias::new("revision"))
					.sum()
					.cast_as(Alias::new("bigint")),
			)
			.expr(Expr::col(Alias::new("revision")).count())
			.from(Alias::new("authorization_catalog"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&tenant)
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
		&tenant_resources("artifacts", "workspace_id")
			.expr(Expr::col((Alias::new("r"), Alias::new("created_at"))).max())
			.expr(Expr::col((Alias::new("r"), Alias::new("id"))).count())
			.to_string(PostgresQueryBuilder),
	)
	.bind(&tenant)
	.fetch_one(&mut *conn)
	.await?;
	let conversations: (Option<DateTime<Utc>>, i64) = sqlx::query_as(
		&tenant_resources("conversations", "workspace_id")
			.expr(Expr::col((Alias::new("r"), Alias::new("created_at"))).max())
			.expr(Expr::col((Alias::new("r"), Alias::new("id"))).count())
			.to_string(PostgresQueryBuilder),
	)
	.bind(&tenant)
	.fetch_one(&mut *conn)
	.await?;
	let events: Option<i64> = sqlx::query_scalar(
		&tenant_resources("events", "workspace_id")
			.expr(Expr::col((Alias::new("r"), Alias::new("sequence"))).max())
			.to_string(PostgresQueryBuilder),
	)
	.bind(&tenant)
	.fetch_one(&mut *conn)
	.await?;
	Ok(crate::registry::digest(&json!({
		"authority":authority_revision,"catalog":catalog,"workspaces":workspaces,
		"tasks":tasks,"runs":runs,"artifacts":artifacts,"conversations":conversations,"events":events,
	})))
}

fn registry_ref(value: &serde_json::Value) -> Option<(&str, &str)> {
	let id = value.get("id")?.as_str()?;
	let version = value.get("version")?.as_str()?;
	if id.is_empty() || version.is_empty() {
		None
	} else {
		Some((id, version))
	}
}

fn build_edges(
	node: &str,
	records: &[Candidate],
	nodes: &[GraphNode],
	options: &GraphOptions,
) -> Vec<GraphEdge> {
	let visible: BTreeSet<&str> = nodes.iter().map(|item| item.id.as_str()).collect();
	let mut edges = BTreeMap::<(String, String, String), GraphEdge>::new();
	let mut add = |source: String, target: String, relation: &str, layer: &str| {
		if source != target
			&& visible.contains(source.as_str())
			&& visible.contains(target.as_str())
			&& options.relations.iter().any(|allowed| allowed == relation)
		{
			edges.insert(
				(source.clone(), relation.to_owned(), target.clone()),
				GraphEdge {
					source,
					target,
					relation: relation.into(),
					layer: layer.into(),
				},
			);
		}
	};
	for record in records {
		match record {
			Candidate::Workspace(workspace) => add(
				resource_key(node, "workspace", workspace.id),
				resource_key(node, "goal", workspace.id),
				"goal",
				"configuration",
			),
			Candidate::Task(task) => {
				let id = resource_key(node, "task", task.id);
				add(
					resource_key(node, "workspace", task.workspace_id),
					id.clone(),
					"contains",
					"configuration",
				);
				if let Some(parent) = task.parent_id {
					add(
						resource_key(node, "task", parent),
						id.clone(),
						"contains",
						"activity",
					);
				} else {
					add(
						resource_key(node, "goal", task.workspace_id),
						id.clone(),
						"contains",
						"activity",
					);
				}
				for dep in &task.dependencies {
					add(
						resource_key(node, "task", dep),
						id.clone(),
						"depends",
						"activity",
					);
				}
			}
			Candidate::Run(run) => {
				let id = resource_key(node, "run", run.id);
				add(
					entity_key(node, "agent", &run.agent_id, &run.agent_version),
					id.clone(),
					"executes",
					"activity",
				);
				add(
					id,
					resource_key(node, "task", run.task_id),
					"executes",
					"activity",
				);
			}
			Candidate::Artifact(artifact) => add(
				resource_key(node, "task", artifact.task_id),
				resource_key(node, "artifact", artifact.id),
				"produces",
				"activity",
			),
			Candidate::Conversation(conversation) => {
				let id = resource_key(node, "conversation", conversation.id);
				add(
					resource_key(node, "workspace", conversation.workspace_id),
					id.clone(),
					"contains",
					"configuration",
				);
				if matches!(conversation.target_kind.as_str(), "agent" | "cluster")
					&& let Some((target, version)) = conversation.target.rsplit_once('@')
				{
					add(
						entity_key(node, &conversation.target_kind, target, version),
						id,
						"participates",
						"activity",
					);
				}
			}
			Candidate::Registry(entry) => {
				let id = entity_key(node, &entry.kind, &entry.id, &entry.version);
				if entry.kind == "cluster"
					&& let Some((target, version)) = registry_ref(&entry.config["coordinator"])
				{
					add(
						id.clone(),
						entity_key(node, "agent", target, version),
						"coordinates",
						"configuration",
					);
				}
				if entry.kind == "agent" {
					for (kind, field, relation) in [
						("tool", "tools", "tool"),
						("model", "model", "model"),
						("skill", "skills", "skill"),
						("cluster", "cluster", "member"),
					] {
						let value = &entry.config[field];
						let refs = value
							.as_array()
							.map(|items| items.iter().collect::<Vec<_>>())
							.unwrap_or_else(|| vec![value]);
						for item in refs {
							if let Some((target, version)) = registry_ref(item) {
								add(
									id.clone(),
									entity_key(node, kind, target, version),
									relation,
									"configuration",
								);
							}
						}
					}
				}
			}
		}
	}
	edges.into_values().take(600).collect()
}

fn event_reference(event: &Event, node: &str) -> Option<String> {
	let id = |value: &serde_json::Value| value.as_str().and_then(|text| text.parse::<Uuid>().ok());
	let data = &event.data;
	if event.kind.starts_with("run.") {
		return id(&data["run_id"])
			.or_else(|| id(&data["id"]))
			.map(|id| resource_key(node, "run", id));
	}
	let workspace = event.workspace_id?;
	if event.kind.starts_with("task.") {
		return id(&data["task"]["id"])
			.or_else(|| id(&data["task_id"]))
			.or_else(|| id(&data["id"]))
			.map(|id| resource_key(node, "task", id));
	}
	if event.kind.starts_with("artifact.") {
		return id(&data["id"]).map(|id| resource_key(node, "artifact", id));
	}
	if event.kind.starts_with("conversation.") {
		return id(&data["id"]).map(|id| resource_key(node, "conversation", id));
	}
	if event.kind.starts_with("message.") || event.kind.starts_with("workspace.") {
		return Some(resource_key(node, "workspace", workspace));
	}
	None
}

async fn project_activity(
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
	let mut query = Query::select();
	query
		.column((Alias::new("e"), Asterisk))
		.from_as(Alias::new("events"), Alias::new("e"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("authorization_workspaces"),
			Alias::new("a"),
			Expr::col((Alias::new("e"), Alias::new("workspace_id")))
				.eq(Expr::col((Alias::new("a"), Alias::new("workspace_id")))),
		)
		.and_where(Expr::col((Alias::new("a"), Alias::new("tenant"))).eq(Expr::cust("$1")))
		.and_where(Expr::col((Alias::new("e"), Alias::new("node_id"))).eq(Expr::cust("$2")))
		.and_where(Expr::col((Alias::new("e"), Alias::new("created_at"))).lte(Expr::cust("$3")))
		.and_where(Expr::cust("e.workspace_id = ANY($4)"))
		.and_where(Expr::cust("($5::bigint IS NULL OR e.sequence < $5)"))
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
	if options.hours > 0 {
		query.and_where(
			Expr::col((Alias::new("e"), Alias::new("created_at"))).gte(Expr::cust("$6")),
		);
	}
	let sql = query.to_string(PostgresQueryBuilder);
	let end = DateTime::<Utc>::from_timestamp(window_end, 0).ok_or(Error::Forbidden)?;
	let start = if options.hours > 0 {
		Some(
			DateTime::<Utc>::from_timestamp(window_end - i64::from(options.hours) * 3600, 0)
				.ok_or(Error::Forbidden)?,
		)
	} else {
		None
	};
	let mut markers = Vec::new();
	let mut before = None;
	let mut scanned = 0;
	loop {
		let mut query = sqlx::query_as::<_, Event>(&sql)
			.bind(&tenant)
			.bind(&f.config.node_id)
			.bind(end)
			.bind(&workspaces)
			.bind(before);
		if let Some(start) = start {
			query = query.bind(start);
		}
		let events = query.fetch_all(authority.connection()).await?;
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
	let generation =
		graph_generation(f, authority, authority_revision, options, source_node).await?;
	let binding = cursor_binding(source_node, viewer, options)?;
	let mut cursor = match &options.cursor {
		Some(token) => decode_cursor(f, token, &binding, &generation)?,
		None => {
			let now = Utc::now().timestamp();
			GraphCursor {
				kind: 0,
				offset: 0,
				generation: generation.clone(),
				binding: binding.clone(),
				window_end: now,
				expires_at: now + 300,
			}
		}
	};
	let mut nodes: Vec<GraphNode> = Vec::new();
	let mut records: Vec<Candidate> = Vec::new();
	let mut bytes = 0_usize;
	let mut scanned = 0_u64;
	let mut next_cursor = None;
	while cursor.kind < CANDIDATE_KINDS {
		if scanned >= 4096 {
			next_cursor = Some(encode_cursor(f, &cursor)?);
			break;
		}
		let batch = candidates(authority, cursor.kind, cursor.offset, options, source_node).await?;
		if batch.is_empty() {
			cursor.kind += 1;
			cursor.offset = 0;
			continue;
		}
		let exhausted = batch.len() < CANDIDATE_BATCH as usize;
		for candidate in batch {
			if scanned >= 4096 {
				next_cursor = Some(encode_cursor(f, &cursor)?);
				break;
			}
			let before = cursor.offset;
			cursor.offset += 1;
			scanned += 1;
			let allowed_kind = kind_allowed(candidate.kind(), options)
				|| matches!(&candidate, Candidate::Workspace(_)) && kind_allowed("goal", options);
			if !allowed_kind {
				continue;
			}
			let visible = match &candidate {
				Candidate::Run(run) if options.scope_workspace.is_some() => {
					scoped::visible(f, authority, run).await?
				}
				_ => authority.visible(&candidate).await?,
			};
			if !visible {
				continue;
			}
			if let Candidate::Run(run) = &candidate
				&& options.hours > 0
				&& run.phase().is_terminal()
				&& run.updated_at.timestamp() < cursor.window_end - i64::from(options.hours) * 3600
			{
				continue;
			}
			let addition: Vec<_> = candidate
				.nodes(&f.config.node_id, options)
				.into_iter()
				.filter(|node| !nodes.iter().any(|existing| existing.id == node.id))
				.collect();
			if addition.is_empty() {
				continue;
			}
			let mut group = addition;
			let mut group_records = vec![candidate.clone()];
			for linked in
				linked_candidates(authority, &candidate, options, &f.config.node_id, &nodes).await?
			{
				let addition: Vec<_> = linked
					.nodes(&f.config.node_id, options)
					.into_iter()
					.filter(|node| {
						!nodes
							.iter()
							.chain(group.iter())
							.any(|existing| existing.id == node.id)
					})
					.collect();
				if group.len() + addition.len() > options.limit as usize {
					continue;
				}
				group.extend(addition);
				group_records.push(linked);
			}
			let size = serde_json::to_vec(&group)?.len();
			if nodes.len() + group.len() > options.limit as usize || bytes + size > 3_000_000 {
				if nodes.is_empty() {
					return Err(Error::Invalid(
						"graph resource exceeds response limit".into(),
					));
				}
				cursor.offset = before;
				next_cursor = Some(encode_cursor(f, &cursor)?);
				break;
			}
			bytes += size;
			nodes.extend(group);
			records.extend(group_records);
		}
		if next_cursor.is_some() {
			break;
		}
		if exhausted {
			cursor.kind += 1;
			cursor.offset = 0;
		}
	}
	let edges = build_edges(&f.config.node_id, &records, &nodes, options);
	let activity = project_activity(
		f,
		authority,
		options,
		&nodes,
		cursor.window_end,
		source_node,
	)
	.await?;
	if graph_generation(f, authority, authority_revision, options, source_node).await? != generation
	{
		return Err(Error::Conflict("graph projection changed".into()));
	}
	let page = GraphPage {
		node_id: f.config.node_id.clone(),
		generation,
		checked_at: Utc::now(),
		nodes,
		edges,
		activity,
		next_cursor,
	};
	if serde_json::to_vec(&page)?.len() > 3_000_000 {
		return Err(Error::Invalid("graph response exceeds size limit".into()));
	}
	Ok(page)
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
				let mapping_revision: i64 = sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("revision"))
						.from(Alias::new("authorization_peer_mappings"))
						.and_where(Expr::cust(
							"source_node = $1 AND source_tenant = $2 AND source_subject = $3 AND enabled",
						))
						.to_string(PostgresQueryBuilder),
				)
				.bind(node)
				.bind(tenant)
				.bind(subject)
				.fetch_one(&mut **access.tx)
				.await?;
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

#[utoipa::path(get,path="/authorization/{tenant}/graph-operator-grants",operation_id="authorization_graph_operator_grants",params(("tenant"=String,Path),GrantPage),responses((status=200,body=[GraphOperatorGrant])),security(("bearer_auth"=[])))]
async fn list_grants(
	State(f): State<Federation>,
	Path(tenant): Path<String>,
	QueryParams(page): QueryParams<GrantPage>,
) -> Result<Json<Vec<GraphOperatorGrant>>> {
	identifier(&tenant)?;
	if !(1..=200).contains(&page.limit) {
		return Err(Error::Invalid("invalid grant page".into()));
	}
	let grants = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("authorization_graph_operator_grants"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.order_by(Alias::new("source_node"), Order::Asc)
			.order_by(Alias::new("source_operator"), Order::Asc)
			.limit(page.limit)
			.offset(page.offset)
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.fetch_all(&f.store.pool)
	.await?;
	Ok(Json(grants))
}

#[utoipa::path(post,path="/authorization/{tenant}/graph-operator-grants",operation_id="authorization_set_graph_operator_grant",params(("tenant"=String,Path)),request_body=GraphOperatorGrantInput,responses((status=200,body=GraphOperatorGrant)),security(("bearer_auth"=[])))]
async fn set_grant(
	State(f): State<Federation>,
	Path(tenant): Path<String>,
	Json(input): Json<GraphOperatorGrantInput>,
) -> Result<Json<GraphOperatorGrant>> {
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
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("$3"),
					Expr::cust("$4"),
					Expr::val(1_i64).into(),
				])
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.returning_all()
				.to_string(PostgresQueryBuilder),
		)
		.bind(&input.source_node)
		.bind(input.source_operator)
		.bind(&tenant)
		.bind(input.enabled)
		.fetch_optional(&mut *tx)
		.await?
	} else {
		sqlx::query_as(
			&Query::update()
				.table(Alias::new("authorization_graph_operator_grants"))
				.value(Alias::new("enabled"), Expr::cust("$4"))
				.value(Alias::new("revision"), Expr::cust("revision + 1"))
				.value(Alias::new("updated_at"), Expr::cust("clock_timestamp()"))
				.and_where(Expr::cust(
					"source_node = $1 AND source_operator = $2 AND tenant = $3 AND revision = $5",
				))
				.returning_all()
				.to_string(PostgresQueryBuilder),
		)
		.bind(&input.source_node)
		.bind(input.source_operator)
		.bind(&tenant)
		.bind(input.enabled)
		.bind(input.expected_revision)
		.fetch_optional(&mut *tx)
		.await?
	};
	let grant =
		row.ok_or_else(|| Error::Conflict("graph operator grant revision changed".into()))?;
	tx.commit().await?;
	Ok(Json(grant))
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
	let grant: GraphOperatorGrant = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("authorization_graph_operator_grants"))
			.and_where(Expr::cust(
				"source_node = $1 AND source_operator = $2 AND tenant = $3 AND enabled",
			))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(node)
	.bind(operator)
	.bind(tenant)
	.fetch_optional(&mut *tx)
	.await?
	.ok_or(Error::Forbidden)?;
	let enabled: Option<String> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("node_id"))
			.from(Alias::new("peers"))
			.and_where(Expr::cust("node_id = $1 AND enabled"))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(node)
	.fetch_optional(&mut *tx)
	.await?;
	if enabled.is_none() {
		return Err(Error::Forbidden);
	}
	Ok((tx, grant))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rejects_multihop_graph_requests() {
		let options = GraphOptions {
			scope_workspace: None,
			depth: 2,
			mode: "mesh".into(),
			kinds: vec!["agent".into()],
			relations: vec!["hosts".into()],
			hours: 24,
			limit: 20,
			cursor: None,
			target_tenant: None,
		};
		assert!(options.validate().is_err());
	}

	#[test]
	fn source_rejects_cross_node_and_dangling_projection() {
		let input = GraphExpandInput {
			node_id: "aidash://b".into(),
			options: GraphOptions {
				scope_workspace: None,
				depth: 1,
				mode: "mesh".into(),
				kinds: vec!["agent".into()],
				relations: vec!["hosts".into()],
				hours: 24,
				limit: 20,
				cursor: None,
				target_tenant: None,
			},
		};
		let mut page = GraphPage {
			node_id: "aidash://b".into(),
			generation: format!("sha256:{}", "a".repeat(64)),
			checked_at: Utc::now(),
			nodes: vec![GraphNode {
				id: entity_key("aidash://b", "agent", "one", "1.0.0"),
				node_id: "aidash://b".into(),
				kind: "agent".into(),
				name: BTreeMap::from([("en".into(), "One".into())]),
				resource_id: Some("one".into()),
				version: Some("1.0.0".into()),
				workspace_id: None,
				status: None,
				goal_body: None,
				at: None,
			}],
			edges: vec![],
			activity: vec![],
			next_cursor: None,
		};
		assert!(valid_remote_page(&page, &input));
		page.nodes[0].id = entity_key("aidash://c", "agent", "one", "1.0.0");
		assert!(!valid_remote_page(&page, &input));
		page.nodes[0].id = entity_key("aidash://b", "agent", "one", "1.0.0");
		page.edges.push(GraphEdge {
			source: page.nodes[0].id.clone(),
			target: entity_key("aidash://b", "agent", "hidden", "1.0.0"),
			relation: "hosts".into(),
			layer: "federation".into(),
		});
		assert!(!valid_remote_page(&page, &input));
	}
}
