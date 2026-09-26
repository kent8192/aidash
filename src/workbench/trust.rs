//! Factual inspection only. No Trust verdict or certification is inferred.
use super::*;
use crate::{authorization::access::Access, domain::Run, registry::EntityRef};
use axum::{
	extract::Query as AxumQuery,
	response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WorkspaceUse {
	pub workspace_id: Uuid,
	pub title: String,
	pub current: bool,
	pub latest_run_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Inspection {
	pub entry: Entry,
	pub source_node: String,
	pub observed_at: DateTime<Utc>,
	pub workspaces: Vec<WorkspaceUse>,
	pub usage_truncated: bool,
	pub test_evidence: Vec<TestEvidence>,
	pub test_evidence_truncated: bool,
	pub external_assessment_available: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TestEvidence {
	pub session_id: Uuid,
	pub draft_revision: i64,
	pub mode: String,
	pub profile_id: Option<String>,
	pub profile_revision: Option<i64>,
	pub status: String,
	pub usage: Value,
	pub created_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub expired_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct EvidenceRow {
	id: Uuid,
	status: String,
	scenario: Value,
	usage: Value,
	created_at: DateTime<Utc>,
	expires_at: DateTime<Utc>,
	expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Report {
	pub inspection: Inspection,
	pub permission_context: Option<PermissionContext>,
	pub incidents: Vec<crate::workbench::incident::Incident>,
	pub exported_at: DateTime<Utc>,
	pub note: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionInput {
	pub tenant: String,
	pub subject: String,
	pub workspace_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PermissionRow {
	pub reference: EntityRef,
	pub kind: String,
	pub action: String,
	pub catalog_enabled: bool,
	pub policy_allowed: bool,
	pub registry_read_allowed: Option<bool>,
	pub effective_for_component: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PermissionContext {
	pub tenant: String,
	pub subject: String,
	pub workspace_id: Option<Uuid>,
	pub policy_revision: i64,
	pub observed_at: DateTime<Utc>,
	pub requested_capabilities: Vec<String>,
	pub rows: Vec<PermissionRow>,
	pub workspace_read: Option<bool>,
	pub note: String,
}

#[derive(Deserialize)]
struct ReportQuery {
	format: Option<String>,
	#[serde(default)]
	include_sensitive: bool,
	tenant: Option<String>,
	subject: Option<String>,
	workspace_id: Option<Uuid>,
}

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(inspect))
		.routes(routes!(report))
		.routes(routes!(permission_context))
}

pub(super) async fn require_inspection(
	tx: &mut Transaction<'_, Postgres>,
	actor: &Actor,
	reference: &EntityRef,
) -> Result<()> {
	let Actor::Subject(identity) = actor else {
		return Ok(());
	};
	let decision = Authorization::evaluate_in_transaction(
		tx,
		&identity.tenant,
		&Evaluation {
			subject: identity.subject.clone(),
			action: "agent_version.inspect".into(),
			resource: Resource {
				tenant: identity.tenant.clone(),
				kind: "agent_version".into(),
				id: ref_key(reference),
				attributes: json!({"agent_id":reference.id,"version":reference.version}),
			},
			environment: json!({}),
		},
	)
	.await?;
	if decision.allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

// Inspection and contained resource checks share one audit allocation lease.
// Opening another audited transaction while this one is live would wait on
// the inspection's own advisory lock.
enum InspectionLease {
	Operator(Transaction<'static, Postgres>),
	Subject(Box<Access>),
}

impl InspectionLease {
	async fn begin(f: &Federation, actor: &Actor) -> Result<Self> {
		match actor {
			Actor::Operator => Ok(Self::Operator(f.store.pool.begin().await?)),
			Actor::Subject(identity) => Ok(Self::Subject(Box::new(
				Access::begin(&f.store, identity).await?,
			))),
		}
	}

	fn tx(&mut self) -> &mut Transaction<'static, Postgres> {
		match self {
			Self::Operator(tx) => tx,
			Self::Subject(access) => &mut access.tx,
		}
	}

	async fn run_visible(&mut self, run: &Run) -> Result<bool> {
		match self {
			Self::Operator(_) => Ok(true),
			Self::Subject(access) => {
				let result = async {
					let workspace = access.workspace(run.workspace_id).await?;
					if !access.decide(&workspace, "workspace.read").await? {
						return Ok(false);
					}
					access.run_visible(run).await
				}
				.await;
				match result {
					Err(Error::Forbidden) => Ok(false),
					other => other,
				}
			}
		}
	}

	async fn finish(self, inspection: Inspection) -> Result<Inspection> {
		match self {
			Self::Operator(tx) => {
				tx.commit().await?;
				Ok(inspection)
			}
			Self::Subject(access) => (*access).finish(Ok(inspection)).await,
		}
	}
}

async fn visible_runs(lease: &mut InspectionLease, reference: &EntityRef) -> Result<Vec<Run>> {
	let mut visible = Vec::new();
	let mut cursor: Option<(DateTime<Utc>, Uuid)> = None;
	loop {
		let mut query = Query::select();
		query
			.expr(Expr::cust("*"))
			.from(Alias::new("runs"))
			.and_where(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("agent_version")).eq(Expr::cust("$2")))
			.order_by(Alias::new("updated_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(501);
		if cursor.is_some() {
			query.cond_where(
				Condition::any()
					.add(Expr::col(Alias::new("updated_at")).lt(Expr::cust("$3")))
					.add(
						Condition::all()
							.add(Expr::col(Alias::new("updated_at")).eq(Expr::cust("$3")))
							.add(Expr::col(Alias::new("id")).lt(Expr::cust("$4"))),
					),
			);
		}
		let sql = query.to_string(PostgresQueryBuilder);
		let mut select = sqlx::query_as::<_, Run>(&sql)
			.bind(&reference.id)
			.bind(&reference.version);
		if let Some((updated_at, id)) = cursor {
			select = select.bind(updated_at).bind(id);
		}
		let rows = select.fetch_all(&mut **lease.tx()).await?;
		let more = rows.len() == 501;
		cursor = rows.last().map(|run| (run.updated_at, run.id));
		for run in rows {
			if lease.run_visible(&run).await? {
				visible.push(run);
			}
			if visible.len() == 501 {
				break;
			}
		}
		if visible.len() == 501 || !more {
			break;
		}
	}
	Ok(visible)
}

#[utoipa::path(get,path="/workbench/versions/{id}/{version}",operation_id="workbench_inspect_version",params(("id"=String,Path),("version"=String,Path)),responses((status=200,body=Inspection)),security(("bearer_auth"=[])))]
async fn inspect(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, version)): Path<(String, String)>,
) -> Result<Json<Inspection>> {
	let reference = EntityRef { id, version };
	let mut lease = InspectionLease::begin(&f, &actor).await?;
	require_inspection(lease.tx(), &actor, &reference).await?;
	let entry = f.registry.get(&reference.id, &reference.version).await?;
	if entry.kind != "agent" {
		return Err(Error::NotFound("agent version".into()));
	}
	let runs = visible_runs(&mut lease, &reference).await?;
	let truncated = runs.len() > 500;
	let mut uses: std::collections::BTreeMap<Uuid, WorkspaceUse> = Default::default();
	for run in runs.into_iter().take(500) {
		add_use(lease.tx(), &mut uses, &run).await?;
	}
	let mut test_evidence = Vec::new();
	let mut test_evidence_truncated = false;
	let registrations: Vec<(Uuid, i64)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("draft_id"), Alias::new("revision")])
			.from(Alias::new("agent_draft_registrations"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("version")).eq(Expr::cust("$2"))),
			)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&reference.id)
	.bind(&reference.version)
	.fetch_all(&mut **lease.tx())
	.await?;
	for (draft_id, revision) in registrations {
		let draft = load(lease.tx(), draft_id, false).await?;
		match authorize(lease.tx(), &actor, &draft, "agent_draft.read", true).await {
			Ok(()) => {}
			Err(Error::Forbidden) => continue,
			Err(error) => return Err(error),
		}
		let sessions: Vec<EvidenceRow> = sqlx::query_as(
			&Query::select()
				.columns([
					Alias::new("id"),
					Alias::new("status"),
					Alias::new("scenario"),
					Alias::new("usage"),
					Alias::new("created_at"),
					Alias::new("expires_at"),
					Alias::new("expired_at"),
				])
				.from(Alias::new("agent_test_sessions"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("revision")).eq(Expr::cust("$2"))),
				)
				.order_by(Alias::new("created_at"), Order::Desc)
				.limit(101)
				.to_string(PostgresQueryBuilder),
		)
		.bind(draft_id)
		.bind(revision)
		.fetch_all(&mut **lease.tx())
		.await?;
		test_evidence_truncated = sessions.len() > 100;
		for session in sessions.into_iter().take(100) {
			test_evidence.push(TestEvidence {
				session_id: session.id,
				draft_revision: revision,
				mode: session.scenario["mode"]
					.as_str()
					.unwrap_or("unknown")
					.into(),
				profile_id: session.scenario["profile_id"].as_str().map(str::to_owned),
				profile_revision: session.scenario["profile_revision"].as_i64(),
				status: session.status,
				usage: session.usage,
				created_at: session.created_at,
				expires_at: session.expires_at,
				expired_at: session.expired_at,
			});
		}
	}
	let inspection = Inspection {
		entry,
		source_node: f.config.node_id,
		observed_at: Utc::now(),
		workspaces: uses.into_values().collect(),
		usage_truncated: truncated,
		test_evidence,
		test_evidence_truncated,
		external_assessment_available: false,
	};
	lease.finish(inspection).await.map(Json)
}

async fn add_use(
	tx: &mut Transaction<'_, Postgres>,
	uses: &mut std::collections::BTreeMap<Uuid, WorkspaceUse>,
	run: &Run,
) -> Result<()> {
	let active = !matches!(run.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED");
	if let Some(existing) = uses.get_mut(&run.workspace_id) {
		existing.current |= active;
		if run.updated_at > existing.latest_run_at {
			existing.latest_run_at = run.updated_at;
		}
		return Ok(());
	}
	let title: Option<String> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("title"))
			.from(Alias::new("workspaces"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.workspace_id)
	.fetch_optional(&mut **tx)
	.await?;
	if let Some(title) = title {
		uses.insert(
			run.workspace_id,
			WorkspaceUse {
				workspace_id: run.workspace_id,
				title,
				current: active,
				latest_run_at: run.updated_at,
			},
		);
	}
	Ok(())
}

#[utoipa::path(post,path="/workbench/versions/{id}/{version}/permissions",operation_id="workbench_permission_context",params(("id"=String,Path),("version"=String,Path)),request_body=PermissionInput,responses((status=200,body=PermissionContext)),security(("bearer_auth"=[])))]
async fn permission_context(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, version)): Path<(String, String)>,
	Json(input): Json<PermissionInput>,
) -> Result<Json<PermissionContext>> {
	if let Actor::Subject(identity) = &actor
		&& (input.tenant != identity.tenant || input.subject != identity.subject)
	{
		return Err(Error::Forbidden);
	}
	let mut tx = f.store.pool.begin().await?;
	require_inspection(
		&mut tx,
		&actor,
		&EntityRef {
			id: id.clone(),
			version: version.clone(),
		},
	)
	.await?;
	let entry = f.registry.get(&id, &version).await?;
	if entry.kind != "agent" {
		return Err(Error::NotFound("agent version".into()));
	}
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	let mut components = vec![(EntityRef { id, version }, "agent.execute")];
	components.push((config.model, "model.infer"));
	components.extend(
		config
			.skills
			.into_iter()
			.map(|reference| (reference, "skill.use")),
	);
	components.extend(
		config
			.tools
			.into_iter()
			.map(|reference| (reference, "tool.invoke")),
	);
	if let Some(cluster) = config.cluster {
		components.push((cluster, "cluster.execute"));
	}
	let mut rows = Vec::new();
	let mut policy_revision = 0;
	for (reference, action) in components {
		let dependency = f.registry.get(&reference.id, &reference.version).await?;
		let catalog_enabled: Option<bool> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("enabled"))
				.from(Alias::new("authorization_catalog"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("entry_id")).eq(Expr::cust("$2")))
						.add(Expr::col(Alias::new("entry_version")).eq(Expr::cust("$3"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&input.tenant)
		.bind(&reference.id)
		.bind(&reference.version)
		.fetch_optional(&mut *tx)
		.await?;
		let mut evaluation = Evaluation {
			subject: input.subject.clone(),
			action: action.into(),
			resource: Resource {
				tenant: input.tenant.clone(),
				kind: dependency.kind.clone(),
				id: reference.id.clone(),
				attributes: json!({"version":reference.version,"capabilities":dependency.capabilities,"tags":dependency.tags,"languages":dependency.languages,"config":dependency.config}),
			},
			environment: json!({"workspace_id":input.workspace_id,"node_id":f.config.node_id,"transport":"worker"}),
		};
		let decision =
			Authorization::evaluate_in_transaction(&mut tx, &input.tenant, &evaluation).await?;
		let registry_read_allowed = if action == "agent.execute" {
			None
		} else {
			evaluation.action = "registry.read".into();
			Some(
				Authorization::evaluate_in_transaction(&mut tx, &input.tenant, &evaluation)
					.await?
					.allowed,
			)
		};
		policy_revision = decision.revision;
		rows.push(PermissionRow {
			reference,
			kind: dependency.kind,
			action: action.into(),
			catalog_enabled: catalog_enabled == Some(true),
			policy_allowed: decision.allowed,
			registry_read_allowed,
			effective_for_component: catalog_enabled == Some(true)
				&& decision.allowed
				&& registry_read_allowed.unwrap_or(true),
		});
	}
	let workspace_read = if let Some(workspace_id) = input.workspace_id {
		let owner: Option<String> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("owner_subject"))
				.from(Alias::new("authorization_workspaces"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(workspace_id)
		.bind(&input.tenant)
		.fetch_optional(&mut *tx)
		.await?;
		if let Some(owner) = owner {
			let decision = Authorization::evaluate_in_transaction(
				&mut tx,
				&input.tenant,
				&Evaluation {
					subject: input.subject.clone(),
					action: "workspace.read".into(),
					resource: Resource {
						tenant: input.tenant.clone(),
						kind: "workspace".into(),
						id: workspace_id.to_string(),
						attributes: json!({"owner":owner,"workspace_id":workspace_id}),
					},
					environment: json!({"workspace_id":workspace_id,"node_id":f.config.node_id,"transport":"worker"}),
				},
			)
			.await?;
			policy_revision = decision.revision;
			Some(decision.allowed)
		} else {
			Some(false)
		}
	} else {
		None
	};
	tx.commit().await?;
	Ok(Json(PermissionContext { tenant: input.tenant, subject: input.subject, workspace_id: input.workspace_id, policy_revision, observed_at: Utc::now(), requested_capabilities: entry.capabilities, rows, workspace_read, note: "Component-level decisions use the worker execution context and include required Registry reads; task and execution admission require further checks. No universal permission or Trust assessment is implied.".into() }))
}

fn escape_html(value: &str) -> String {
	value
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
}

#[utoipa::path(get,path="/workbench/versions/{id}/{version}/report",operation_id="workbench_export_report",params(("id"=String,Path),("version"=String,Path)),responses((status=200,description="JSON or printable HTML point-in-time factual report")),security(("bearer_auth"=[])))]
async fn report(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, version)): Path<(String, String)>,
	AxumQuery(query): AxumQuery<ReportQuery>,
) -> Result<Response> {
	let format = query.format.as_deref().unwrap_or("json");
	if !matches!(format, "json" | "html") {
		return Err(Error::Invalid("report format must be json or html".into()));
	}
	let inspection = inspect(
		State(f.clone()),
		Extension(actor.clone()),
		Path((id.clone(), version.clone())),
	)
	.await?
	.0;
	let context_input = match &actor {
		Actor::Subject(identity) => {
			if query
				.tenant
				.as_ref()
				.is_some_and(|tenant| tenant != &identity.tenant)
				|| query
					.subject
					.as_ref()
					.is_some_and(|subject| subject != &identity.subject)
			{
				return Err(Error::Forbidden);
			}
			Some(PermissionInput {
				tenant: identity.tenant.clone(),
				subject: identity.subject.clone(),
				workspace_id: query.workspace_id,
			})
		}
		Actor::Operator => match (&query.tenant, &query.subject) {
			(Some(tenant), Some(subject)) => Some(PermissionInput {
				tenant: tenant.clone(),
				subject: subject.clone(),
				workspace_id: query.workspace_id,
			}),
			(None, None) if query.workspace_id.is_none() => None,
			_ => {
				return Err(Error::Invalid(
					"report permission context requires tenant and subject together".into(),
				));
			}
		},
	};
	let context = if let Some(input) = context_input {
		Some(
			permission_context(
				State(f.clone()),
				Extension(actor.clone()),
				Path((id.clone(), version.clone())),
				Json(input),
			)
			.await?
			.0,
		)
	} else {
		None
	};
	let mut incidents = super::incident::list(State(f), Extension(actor), Path((id, version)))
		.await?
		.0;
	if !query.include_sensitive {
		for incident in &mut incidents {
			if let Some(copies) = incident.evidence.as_array_mut() {
				for copy in copies {
					if let Some(copy) = copy.as_object_mut() {
						copy.remove("content");
					}
				}
			}
		}
	}
	let report = Report {
		inspection,
		permission_context: context,
		incidents,
		exported_at: Utc::now(),
		note: "Factual connected-node snapshot; no Trust assessment or certification is provided."
			.into(),
	};
	if format == "json" {
		return Ok(Json(report).into_response());
	}
	let title = escape_html(&format!(
		"Aidash agent report · {}@{}",
		report.inspection.entry.id, report.inspection.entry.version
	));
	let content = escape_html(&serde_json::to_string_pretty(&report)?);
	let html = format!(
		"<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:14px/1.5 system-ui,sans-serif;max-width:900px;margin:32px auto;color:#172236}}h1{{font-size:24px}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#f4f6fa;padding:20px;border:1px solid #dce2ec}}@media print{{body{{margin:0}}pre{{border:0;padding:0}}}}</style></head><body><h1>{title}</h1><p>Factual connected-node snapshot. No Trust assessment or certification is provided.</p><pre>{content}</pre></body></html>"
	);
	Ok(Html(html).into_response())
}
