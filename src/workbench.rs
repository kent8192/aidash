//! Tenant-scoped Creator drafts and immutable Registry admission. Workbench
//! permissions are separate from installation-wide Registry administration.
use crate::{
	Error, Result,
	authorization::{
		Authorization,
		identity::Actor,
		policy::{Evaluation, Resource},
	},
	federation::Federation,
	knowledge::{ReferenceDocument, digest, validate as validate_documents},
	registry::{AgentConfig, Entry},
};
use axum::{
	Extension, Json,
	extract::{Path, Query as ExtractQuery, State},
};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{Alias, Condition, Expr, OnConflict, Order, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

mod audit;
mod incident;
mod profile;
mod test;
mod trust;
pub use incident::purge_expired as purge_incident_evidence;
pub use test::purge_expired;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Draft {
	pub id: Uuid,
	pub tenant: String,
	pub owner: String,
	pub revision: i64,
	pub entry: Value,
	pub documents: Value,
	pub release_notes: String,
	pub source_id: Option<String>,
	pub source_version: Option<String>,
	pub archived: bool,
	pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDraft {
	/// Operators must specify both fields. Authenticated subjects use their own identity.
	pub tenant: Option<String>,
	pub owner: Option<String>,
	pub entry: Entry,
	#[serde(default)]
	pub documents: Vec<ReferenceDocument>,
	#[serde(default)]
	pub release_notes: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveDraft {
	pub expected_revision: i64,
	pub entry: Entry,
	#[serde(default)]
	pub documents: Vec<ReferenceDocument>,
	#[serde(default)]
	pub release_notes: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionInput {
	pub expected_revision: i64,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareInput {
	pub subject: String,
	pub can_edit: bool,
	pub enabled: bool,
	/// A shared draft always includes its private attachments.
	pub include_documents: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DraftShare {
	pub subject: String,
	pub can_edit: bool,
	pub documents_current: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferInput {
	pub expected_revision: i64,
	pub new_owner: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveInput {
	pub expected_revision: i64,
	pub archived: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AdoptInput {
	pub tenant: String,
	pub owner: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Validation {
	pub draft_id: Uuid,
	pub revision: i64,
	pub valid: bool,
	pub message: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Registration {
	pub draft_id: Uuid,
	pub revision: i64,
	pub entry: Entry,
	pub behavioral_tested: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegisteredVersion {
	pub entry: Entry,
	pub draft_revision: Option<i64>,
	pub registered_by: Option<String>,
	pub registered_at: Option<DateTime<Utc>>,
	pub release_notes: String,
	pub source_id: Option<String>,
	pub source_version: Option<String>,
	pub behavioral_tested: Option<bool>,
}

#[derive(sqlx::FromRow)]
struct RegisteredVersionRow {
	version: String,
	revision: i64,
	actor: String,
	registered_at: DateTime<Utc>,
	release_notes: String,
	source_id: Option<String>,
	source_version: Option<String>,
	behavioral_tested: bool,
}

const DRAFT_COLUMNS: &str = "id, tenant, owner, revision, entry, documents, release_notes, source_id, source_version, archived, updated_at";

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(create))
		.routes(routes!(list))
		.routes(routes!(get))
		.routes(routes!(save))
		.routes(routes!(validate))
		.routes(routes!(register))
		.routes(routes!(versions))
		.routes(routes!(share))
		.routes(routes!(shares))
		.routes(routes!(transfer))
		.routes(routes!(archive))
		.routes(routes!(duplicate))
		.routes(routes!(adopt))
		.merge(trust::routes())
		.merge(profile::routes())
		.merge(test::routes())
		.merge(incident::routes())
		.merge(audit::routes())
}

fn author_identity(
	actor: &Actor,
	tenant: Option<&str>,
	owner: Option<&str>,
) -> Result<(String, String)> {
	match actor {
		Actor::Operator => {
			let tenant = tenant
				.filter(|s| !s.trim().is_empty())
				.ok_or_else(|| Error::Invalid("tenant is required".into()))?;
			let owner = owner
				.filter(|s| !s.trim().is_empty())
				.ok_or_else(|| Error::Invalid("owner is required".into()))?;
			Ok((tenant.into(), owner.into()))
		}
		Actor::Subject(identity) => {
			if tenant.is_some_and(|value| value != identity.tenant)
				|| owner.is_some_and(|value| value != identity.subject)
			{
				return Err(Error::Forbidden);
			}
			Ok((identity.tenant.clone(), identity.subject.clone()))
		}
	}
}

async fn authorize(
	tx: &mut Transaction<'_, Postgres>,
	actor: &Actor,
	draft: &Draft,
	action: &str,
	shares: bool,
) -> Result<()> {
	let Actor::Subject(identity) = actor else {
		return Ok(());
	};
	if identity.tenant != draft.tenant {
		return Err(Error::Forbidden);
	}
	identity.lock_with_mode(tx, false).await?;
	let shared: Option<(bool, String)> = if shares {
		sqlx::query_as(
			&Query::select()
				.columns([Alias::new("can_edit"), Alias::new("documents_digest")])
				.from(Alias::new("agent_draft_shares"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("subject")).eq(Expr::cust("$2"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(draft.id)
		.bind(&identity.subject)
		.fetch_optional(&mut **tx)
		.await?
	} else {
		None
	};
	let current_share =
		shared.filter(|(_, documents_digest)| documents_digest == &digest(&draft.documents));
	if identity.subject != draft.owner
		&& match action {
			"agent_draft.read" => current_share.is_none(),
			_ => current_share.as_ref().is_none_or(|(can_edit, _)| !can_edit),
		} {
		return Err(Error::Forbidden);
	}
	let decision = Authorization::evaluate_in_transaction(
		tx,
		&draft.tenant,
		&Evaluation {
			subject: identity.subject.clone(),
			action: action.into(),
			resource: Resource {
				tenant: draft.tenant.clone(),
				kind: "agent_draft".into(),
				id: draft.id.to_string(),
				attributes: json!({"owner":draft.owner,"agent_id":draft.entry["id"],"archived":draft.archived}),
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

async fn load(tx: &mut Transaction<'_, Postgres>, id: Uuid, lock: bool) -> Result<Draft> {
	let mut query = Query::select();
	query
		.expr(Expr::cust(DRAFT_COLUMNS))
		.from(Alias::new("agent_drafts"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")));
	if lock {
		query.lock(sea_orm::sea_query::LockType::Update);
	}
	sqlx::query_as(&query.to_string(PostgresQueryBuilder))
		.bind(id)
		.fetch_optional(&mut **tx)
		.await?
		.ok_or_else(|| Error::NotFound("agent draft".into()))
}

fn check_content(
	entry: &Entry,
	documents: &[ReferenceDocument],
	release_notes: &str,
) -> Result<()> {
	if entry.kind != "agent" || entry.id.is_empty() || entry.id.len() > 100 {
		return Err(Error::Invalid(
			"draft must contain a managed agent identity".into(),
		));
	}
	if !documents.is_empty() {
		validate_documents(documents)?;
	}
	if release_notes.len() > 8192 {
		return Err(Error::Invalid("release notes exceed 8 KiB".into()));
	}
	let _: AgentConfig =
		serde_json::from_value(entry.config.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
	Ok(())
}

fn new_draft_defaults(entry: &mut Entry) -> Result<()> {
	let config = entry
		.config
		.as_object_mut()
		.ok_or_else(|| Error::Invalid("agent config must be an object".into()))?;
	for key in [
		"allow_task_creation",
		"allow_task_delegation",
		"allow_memory_write",
		"allow_workspace_retrieval",
		"allow_cross_conversation_memory",
	] {
		match config.get(key) {
			Some(Value::Bool(_)) => {}
			None => {
				config.insert(key.into(), json!(false));
			}
			Some(_) => return Err(Error::Invalid(format!("{key} must be a boolean"))),
		}
	}
	Ok(())
}

#[utoipa::path(post,path="/workbench/drafts",operation_id="workbench_create_draft",request_body=CreateDraft,responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn create(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Json(mut input): Json<CreateDraft>,
) -> Result<Json<Draft>> {
	let (tenant, owner) = author_identity(&actor, input.tenant.as_deref(), input.owner.as_deref())?;
	target_enabled(&f, &tenant, &owner).await?;
	let id = Uuid::now_v7();
	if !input.entry.id.is_empty() {
		return Err(Error::Invalid("new draft must leave entry.id empty; use an authorized version flow for existing identities".into()));
	}
	input.entry.id = id.to_string();
	new_draft_defaults(&mut input.entry)?;
	check_content(&input.entry, &input.documents, &input.release_notes)?;
	let entry = serde_json::to_value(&input.entry)?;
	let documents = serde_json::to_value(&input.documents)?;
	let mut tx = f.store.pool.begin().await?;
	let prospective = Draft {
		id,
		tenant,
		owner,
		revision: 1,
		entry: entry.clone(),
		documents: documents.clone(),
		release_notes: input.release_notes.clone(),
		source_id: None,
		source_version: None,
		archived: false,
		updated_at: Utc::now(),
	};
	authorize(&mut tx, &actor, &prospective, "agent_draft.create", false).await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_drafts"))
			.columns([
				Alias::new("id"),
				Alias::new("tenant"),
				Alias::new("owner"),
				Alias::new("managed_id"),
				Alias::new("entry"),
				Alias::new("documents"),
				Alias::new("release_notes"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("$5"),
				Expr::cust("$6"),
				Expr::cust("$7"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&prospective.tenant)
	.bind(&prospective.owner)
	.bind(&input.entry.id)
	.bind(entry)
	.bind(documents)
	.bind(&input.release_notes)
	.execute(&mut *tx)
	.await?;
	let saved = load(&mut tx, id, false).await?;
	tx.commit().await?;
	Ok(Json(saved))
}

#[derive(Debug, Default, Deserialize)]
struct DraftPage {
	before_updated_at: Option<DateTime<Utc>>,
	before_id: Option<Uuid>,
}

#[utoipa::path(get,path="/workbench/drafts",operation_id="workbench_list_drafts",params(("before_updated_at"=Option<DateTime<Utc>>,Query,description="Timestamp of the last draft on the previous page"),("before_id"=Option<Uuid>,Query,description="ID of the last draft on the previous page; requires before_updated_at")),responses((status=200,body=[Draft])),security(("bearer_auth"=[])))]
async fn list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	ExtractQuery(page): ExtractQuery<DraftPage>,
) -> Result<Json<Vec<Draft>>> {
	let mut tx = f.store.pool.begin().await?;
	let mut visible = Vec::new();
	let mut cursor = match (page.before_updated_at, page.before_id) {
		(None, None) => None,
		(Some(updated_at), Some(id)) => Some((updated_at, id)),
		_ => {
			return Err(Error::Invalid(
				"draft cursor requires both timestamp and ID".into(),
			));
		}
	};
	let tenant = match &actor {
		Actor::Operator => None,
		Actor::Subject(identity) => Some(&identity.tenant),
	};
	loop {
		let mut query = Query::select();
		query
			.expr(Expr::cust(DRAFT_COLUMNS))
			.from(Alias::new("agent_drafts"))
			.order_by(Alias::new("updated_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(100)
			.cond_where(
				Condition::any()
					.add(Expr::expr(Expr::cust("$1::text")).is_null())
					.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1"))),
			);
		if cursor.is_some() {
			query.cond_where(
				Condition::any()
					.add(Expr::col(Alias::new("updated_at")).lt(Expr::cust("$2")))
					.add(
						Condition::all()
							.add(Expr::col(Alias::new("updated_at")).eq(Expr::cust("$2")))
							.add(Expr::col(Alias::new("id")).lt(Expr::cust("$3"))),
					),
			);
		}
		let sql = query.to_string(PostgresQueryBuilder);
		let mut select = sqlx::query_as::<_, Draft>(&sql).bind(tenant);
		if let Some((updated_at, id)) = cursor {
			select = select.bind(updated_at).bind(id);
		}
		let rows = select.fetch_all(&mut *tx).await?;
		let more = rows.len() == 100;
		cursor = rows.last().map(|row| (row.updated_at, row.id));
		for row in rows {
			match authorize(&mut tx, &actor, &row, "agent_draft.read", true).await {
				Ok(()) => visible.push(row),
				Err(Error::Forbidden) => {}
				Err(error) => return Err(error),
			}
			if visible.len() == 100 {
				break;
			}
		}
		if visible.len() == 100 || !more {
			break;
		}
	}
	tx.commit().await?;
	Ok(Json(visible))
}

#[utoipa::path(get,path="/workbench/drafts/{id}",operation_id="workbench_get_draft",params(("id"=Uuid,Path)),responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn get(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Draft>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, false).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await?;
	tx.commit().await?;
	Ok(Json(draft))
}

#[utoipa::path(put,path="/workbench/drafts/{id}",operation_id="workbench_save_draft",params(("id"=Uuid,Path)),request_body=SaveDraft,responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn save(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(mut input): Json<SaveDraft>,
) -> Result<Json<Draft>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, true).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.write", true).await?;
	if draft.revision != input.expected_revision {
		return Err(Error::Conflict(
			"draft revision changed; local edits were not saved".into(),
		));
	}
	if draft.archived {
		return Err(Error::Conflict(
			"restore the archived draft before editing".into(),
		));
	}
	if input.entry.id != draft.entry["id"].as_str().unwrap_or_default() {
		return Err(Error::Invalid(
			"managed agent identity cannot change".into(),
		));
	}
	new_draft_defaults(&mut input.entry)?;
	check_content(&input.entry, &input.documents, &input.release_notes)?;
	let entry = serde_json::to_value(&input.entry)?;
	let documents = serde_json::to_value(&input.documents)?;
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_drafts"))
			.value(Alias::new("revision"), Expr::cust("revision + 1"))
			.value(Alias::new("entry"), Expr::cust("$2"))
			.value(Alias::new("documents"), Expr::cust("$3"))
			.value(Alias::new("release_notes"), Expr::cust("$4"))
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(entry)
	.bind(documents)
	.bind(&input.release_notes)
	.execute(&mut *tx)
	.await?;
	let saved = load(&mut tx, id, false).await?;
	tx.commit().await?;
	Ok(Json(saved))
}

async fn validate_content(
	f: &Federation,
	draft: &Draft,
	actor: &Actor,
	tx: &mut Transaction<'_, Postgres>,
) -> Result<Entry> {
	let mut entry: Entry = serde_json::from_value(draft.entry.clone())?;
	let documents: Vec<ReferenceDocument> = serde_json::from_value(draft.documents.clone())?;
	check_content(&entry, &documents, &draft.release_notes)?;
	if !documents.is_empty() {
		entry.config["knowledge_digest"] = json!(digest(&draft.documents));
	} else if let Some(config) = entry.config.as_object_mut() {
		config.remove("knowledge_digest");
	}
	if let Actor::Subject(identity) = actor {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		for reference in std::iter::once(&config.model)
			.chain(config.tools.iter())
			.chain(config.skills.iter())
			.chain(config.cluster.iter())
		{
			let decision = Authorization::evaluate_in_transaction(
				tx,
				&draft.tenant,
				&Evaluation {
					subject: identity.subject.clone(),
					action: "agent_dependency.read".into(),
					resource: Resource {
						tenant: draft.tenant.clone(),
						kind: "registry_entry".into(),
						id: ref_key(reference),
						attributes: json!({"id":reference.id,"version":reference.version}),
					},
					environment: json!({}),
				},
			)
			.await?;
			if !decision.allowed {
				return Err(Error::Forbidden);
			}
		}
	}
	f.registry.validate_references(&entry).await?;
	if !documents.is_empty() {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		let mut references = Vec::new();
		for reference in std::iter::once(&config.model)
			.chain(config.tools.iter())
			.chain(config.skills.iter())
			.chain(config.cluster.iter())
		{
			references.push(f.registry.get(&reference.id, &reference.version).await?);
		}
		crate::registry::validate_agent_prompt(
			&config,
			&references,
			&json!({"reference_documents":draft.documents}),
		)?;
	}
	Ok(entry)
}

fn ref_key(reference: &crate::registry::EntityRef) -> String {
	format!("{}@{}", reference.id, reference.version)
}

async fn target_enabled(f: &Federation, tenant: &str, subject: &str) -> Result<()> {
	let snapshot = (Authorization {
		pool: f.store.pool.clone(),
	})
	.snapshot(tenant)
	.await?;
	if crate::authorization::identity::enabled(&snapshot, subject) {
		Ok(())
	} else {
		Err(Error::Invalid(
			"target subject must exist and be enabled in the same tenant".into(),
		))
	}
}

#[utoipa::path(post,path="/workbench/drafts/{id}/duplicate",operation_id="workbench_duplicate_draft",params(("id"=Uuid,Path)),request_body=RevisionInput,responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn duplicate(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<RevisionInput>,
) -> Result<Json<Draft>> {
	let mut tx = f.store.pool.begin().await?;
	let original = load(&mut tx, id, true).await?;
	authorize(&mut tx, &actor, &original, "agent_draft.read", true).await?;
	if original.revision != input.expected_revision {
		return Err(Error::Conflict("draft revision changed".into()));
	}
	let id = Uuid::now_v7();
	let mut entry: Entry = serde_json::from_value(original.entry.clone())?;
	let source_id = entry.id.clone();
	let source_version = entry.version.clone();
	entry.id = id.to_string();
	entry.version = "1.0.0".into();
	new_draft_defaults(&mut entry)?;
	let prospective = Draft {
		id,
		tenant: original.tenant.clone(),
		owner: match &actor {
			Actor::Operator => original.owner.clone(),
			Actor::Subject(identity) => identity.subject.clone(),
		},
		revision: 1,
		entry: serde_json::to_value(&entry)?,
		documents: original.documents.clone(),
		release_notes: String::new(),
		source_id: Some(source_id),
		source_version: Some(source_version),
		archived: false,
		updated_at: Utc::now(),
	};
	authorize(&mut tx, &actor, &prospective, "agent_draft.create", false).await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_drafts"))
			.columns([
				Alias::new("id"),
				Alias::new("tenant"),
				Alias::new("owner"),
				Alias::new("managed_id"),
				Alias::new("entry"),
				Alias::new("documents"),
				Alias::new("release_notes"),
				Alias::new("source_id"),
				Alias::new("source_version"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("$5"),
				Expr::cust("$6"),
				Expr::cust("$7"),
				Expr::cust("$8"),
				Expr::cust("$9"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&prospective.tenant)
	.bind(&prospective.owner)
	.bind(&entry.id)
	.bind(&prospective.entry)
	.bind(&prospective.documents)
	.bind("")
	.bind(&prospective.source_id)
	.bind(&prospective.source_version)
	.execute(&mut *tx)
	.await?;
	let copied = load(&mut tx, id, false).await?;
	f.store.event(&mut tx, None, "agent_draft.duplicated", json!({"draft_id":id,"source_id":prospective.source_id,"source_version":prospective.source_version,"tenant":prospective.tenant})).await?;
	tx.commit().await?;
	Ok(Json(copied))
}

#[utoipa::path(post,path="/workbench/agents/{id}/{version}/adopt",operation_id="workbench_adopt_agent",params(("id"=String,Path),("version"=String,Path)),request_body=AdoptInput,responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn adopt(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, version)): Path<(String, String)>,
	Json(input): Json<AdoptInput>,
) -> Result<Json<Draft>> {
	if !matches!(actor, Actor::Operator) {
		return Err(Error::Forbidden);
	}
	let (tenant, owner) = author_identity(&actor, Some(&input.tenant), Some(&input.owner))?;
	target_enabled(&f, &tenant, &owner).await?;
	let mut entry = f.registry.get(&id, &version).await?;
	if entry.kind != "agent" {
		return Err(Error::Invalid(
			"only agents can be assigned to Creator".into(),
		));
	}
	let documents = crate::knowledge::load(&f.registry.db, &entry).await?;
	new_draft_defaults(&mut entry)?;
	let mut tx = f.store.pool.begin().await?;
	let existing: Option<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("agent_drafts"))
			.and_where(Expr::col(Alias::new("managed_id")).eq(Expr::cust("$1")))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&id)
	.fetch_optional(&mut *tx)
	.await?;
	if existing.is_some() {
		return Err(Error::Conflict("agent identity is already managed".into()));
	}
	let draft_id = Uuid::now_v7();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_drafts"))
			.columns([
				Alias::new("id"),
				Alias::new("tenant"),
				Alias::new("owner"),
				Alias::new("managed_id"),
				Alias::new("entry"),
				Alias::new("documents"),
				Alias::new("release_notes"),
				Alias::new("source_id"),
				Alias::new("source_version"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("$5"),
				Expr::cust("$6"),
				Expr::cust("$7"),
				Expr::cust("$8"),
				Expr::cust("$9"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(draft_id)
	.bind(&tenant)
	.bind(&owner)
	.bind(&id)
	.bind(serde_json::to_value(&entry)?)
	.bind(documents)
	.bind("")
	.bind(&id)
	.bind(&version)
	.execute(&mut *tx)
	.await?;
	f.store
		.event(
			&mut tx,
			None,
			"agent_draft.adopted",
			json!({"draft_id":draft_id,"agent_id":id,"version":version,"tenant":tenant,"owner":owner}),
		)
		.await?;
	let draft = load(&mut tx, draft_id, false).await?;
	tx.commit().await?;
	Ok(Json(draft))
}

fn owner_only(actor: &Actor, draft: &Draft) -> Result<()> {
	match actor {
		Actor::Operator => Ok(()),
		Actor::Subject(identity)
			if identity.tenant == draft.tenant && identity.subject == draft.owner =>
		{
			Ok(())
		}
		_ => Err(Error::Forbidden),
	}
}

#[utoipa::path(post,path="/workbench/drafts/{id}/shares",operation_id="workbench_share_draft",params(("id"=Uuid,Path)),request_body=ShareInput,responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn share(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<ShareInput>,
) -> Result<Json<Draft>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, true).await?;
	owner_only(&actor, &draft)?;
	authorize(&mut tx, &actor, &draft, "agent_draft.share", false).await?;
	if draft.owner == input.subject {
		return Err(Error::Invalid("owner does not need a share".into()));
	}
	if input.enabled && draft.documents != json!([]) && !input.include_documents {
		return Err(Error::Invalid(
			"sharing this draft also shares its private documents; acknowledge include_documents"
				.into(),
		));
	}
	if input.enabled {
		target_enabled(&f, &draft.tenant, &input.subject).await?;
	}
	if input.enabled {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("agent_draft_shares"))
				.columns([
					Alias::new("draft_id"),
					Alias::new("subject"),
					Alias::new("can_edit"),
					Alias::new("documents_digest"),
				])
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("$3"),
					Expr::cust("$4"),
				])
				.on_conflict(
					OnConflict::columns([Alias::new("draft_id"), Alias::new("subject")])
						.update_columns([Alias::new("can_edit"), Alias::new("documents_digest")])
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(&input.subject)
		.bind(input.can_edit)
		.bind(digest(&draft.documents))
		.execute(&mut *tx)
		.await?;
	} else {
		sqlx::query(
			&Query::delete()
				.from_table(Alias::new("agent_draft_shares"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("subject")).eq(Expr::cust("$2"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(&input.subject)
		.execute(&mut *tx)
		.await?;
	}
	f.store.event(&mut tx, None, "agent_draft.share_changed", json!({"draft_id":id,"tenant":draft.tenant,"subject":input.subject,"enabled":input.enabled,"can_edit":input.can_edit})).await?;
	tx.commit().await?;
	Ok(Json(draft))
}

#[utoipa::path(get,path="/workbench/drafts/{id}/shares",operation_id="workbench_draft_shares",params(("id"=Uuid,Path)),responses((status=200,body=[DraftShare])),security(("bearer_auth"=[])))]
async fn shares(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Vec<DraftShare>>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, false).await?;
	owner_only(&actor, &draft)?;
	authorize(&mut tx, &actor, &draft, "agent_draft.share", false).await?;
	let rows: Vec<(String, bool, String)> = sqlx::query_as(
		&Query::select()
			.columns([
				Alias::new("subject"),
				Alias::new("can_edit"),
				Alias::new("documents_digest"),
			])
			.from(Alias::new("agent_draft_shares"))
			.and_where(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
			.order_by(Alias::new("subject"), Order::Asc)
			.limit(100)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_all(&mut *tx)
	.await?;
	tx.commit().await?;
	let current_digest = digest(&draft.documents);
	Ok(Json(
		rows.into_iter()
			.map(|(subject, can_edit, documents_digest)| DraftShare {
				subject,
				can_edit,
				documents_current: documents_digest == current_digest,
			})
			.collect(),
	))
}

#[utoipa::path(post,path="/workbench/drafts/{id}/transfer",operation_id="workbench_transfer_draft",params(("id"=Uuid,Path)),request_body=TransferInput,responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn transfer(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<TransferInput>,
) -> Result<Json<Draft>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, true).await?;
	owner_only(&actor, &draft)?;
	authorize(&mut tx, &actor, &draft, "agent_draft.transfer", false).await?;
	if draft.revision != input.expected_revision {
		return Err(Error::Conflict("draft revision changed".into()));
	}
	target_enabled(&f, &draft.tenant, &input.new_owner).await?;
	// Ownership supersedes a share. Retaining it would restore the former
	// owner's access after a later transfer.
	sqlx::query(
		&Query::delete()
			.from_table(Alias::new("agent_draft_shares"))
			.and_where(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("subject")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&input.new_owner)
	.execute(&mut *tx)
	.await?;
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_drafts"))
			.value(Alias::new("owner"), Expr::cust("$2"))
			.value(Alias::new("revision"), Expr::cust("revision + 1"))
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&input.new_owner)
	.execute(&mut *tx)
	.await?;
	f.store
		.event(
			&mut tx,
			None,
			"agent_draft.owner_changed",
			json!({"draft_id":id,"tenant":draft.tenant,"from":draft.owner,"to":input.new_owner}),
		)
		.await?;
	let result = load(&mut tx, id, false).await?;
	tx.commit().await?;
	Ok(Json(result))
}

#[utoipa::path(post,path="/workbench/drafts/{id}/archive",operation_id="workbench_archive_draft",params(("id"=Uuid,Path)),request_body=ArchiveInput,responses((status=200,body=Draft)),security(("bearer_auth"=[])))]
async fn archive(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<ArchiveInput>,
) -> Result<Json<Draft>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, true).await?;
	owner_only(&actor, &draft)?;
	authorize(&mut tx, &actor, &draft, "agent_draft.archive", false).await?;
	if draft.revision != input.expected_revision {
		return Err(Error::Conflict("draft revision changed".into()));
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_drafts"))
			.value(Alias::new("archived"), Expr::cust("$2"))
			.value(Alias::new("revision"), Expr::cust("revision + 1"))
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(input.archived)
	.execute(&mut *tx)
	.await?;
	f.store
		.event(
			&mut tx,
			None,
			"agent_draft.archived_changed",
			json!({"draft_id":id,"tenant":draft.tenant,"archived":input.archived}),
		)
		.await?;
	let result = load(&mut tx, id, false).await?;
	tx.commit().await?;
	Ok(Json(result))
}

#[utoipa::path(post,path="/workbench/drafts/{id}/validate",operation_id="workbench_validate_draft",params(("id"=Uuid,Path)),request_body=RevisionInput,responses((status=200,body=Validation)),security(("bearer_auth"=[])))]
async fn validate(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<RevisionInput>,
) -> Result<Json<Validation>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, false).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.write", true).await?;
	if input.expected_revision != draft.revision {
		return Err(Error::Conflict("draft revision changed".into()));
	}
	// Validation is advisory; Register repeats all checks on the locked revision.
	let result = validate_content(&f, &draft, &actor, &mut tx).await;
	tx.commit().await?;
	Ok(Json(Validation {
		draft_id: id,
		revision: draft.revision,
		valid: result.is_ok(),
		message: result
			.err()
			.map_or_else(|| "Technical validation passed".into(), |e| e.to_string()),
	}))
}

#[utoipa::path(get,path="/workbench/drafts/{id}/versions",operation_id="workbench_draft_versions",params(("id"=Uuid,Path)),responses((status=200,body=[RegisteredVersion])),security(("bearer_auth"=[])))]
async fn versions(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Vec<RegisteredVersion>>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, false).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await?;
	let managed_id = draft.entry["id"]
		.as_str()
		.ok_or_else(|| Error::Invalid("draft has no managed identity".into()))?;
	let rows: Vec<RegisteredVersionRow> = sqlx::query_as(
		&Query::select()
			.columns([
				Alias::new("version"),
				Alias::new("revision"),
				Alias::new("actor"),
				Alias::new("registered_at"),
				Alias::new("release_notes"),
				Alias::new("source_id"),
				Alias::new("source_version"),
				Alias::new("behavioral_tested"),
			])
			.from(Alias::new("agent_draft_registrations"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$2"))),
			)
			.order_by(Alias::new("registered_at"), Order::Desc)
			.limit(100)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(managed_id)
	.fetch_all(&mut *tx)
	.await?;
	let mut versions = Vec::new();
	for row in rows {
		versions.push(RegisteredVersion {
			entry: f.registry.get(managed_id, &row.version).await?,
			draft_revision: Some(row.revision),
			registered_by: Some(row.actor),
			registered_at: Some(row.registered_at),
			release_notes: row.release_notes,
			source_id: row.source_id,
			source_version: row.source_version,
			behavioral_tested: Some(row.behavioral_tested),
		});
	}
	if draft.source_id.as_deref() == Some(managed_id)
		&& let Some(source_version) = &draft.source_version
		&& !versions
			.iter()
			.any(|item| &item.entry.version == source_version)
	{
		versions.push(RegisteredVersion {
			entry: f.registry.get(managed_id, source_version).await?,
			draft_revision: None,
			registered_by: None,
			registered_at: None,
			release_notes: String::new(),
			source_id: None,
			source_version: None,
			behavioral_tested: None,
		});
	}
	tx.commit().await?;
	Ok(Json(versions))
}

#[utoipa::path(post,path="/workbench/drafts/{id}/register",operation_id="workbench_register_draft",params(("id"=Uuid,Path)),request_body=RevisionInput,responses((status=200,body=Registration)),security(("bearer_auth"=[])))]
async fn register(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<RevisionInput>,
) -> Result<Json<Registration>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, true).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.register", true).await?;
	if draft.archived || input.expected_revision != draft.revision {
		return Err(Error::Conflict(
			"draft revision changed or is archived".into(),
		));
	}
	let entry = validate_content(&f, &draft, &actor, &mut tx).await?;
	let registered_revision: Option<(Uuid, i64, bool)> = sqlx::query_as(
		&Query::select()
			.columns([
				Alias::new("draft_id"),
				Alias::new("revision"),
				Alias::new("behavioral_tested"),
			])
			.from(Alias::new("agent_draft_registrations"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("version")).eq(Expr::cust("$2"))),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&entry.id)
	.bind(&entry.version)
	.fetch_optional(&mut *tx)
	.await?;
	if registered_revision
		.as_ref()
		.is_some_and(|(registered_draft, revision, _)| {
			*registered_draft != id || *revision != draft.revision
		}) {
		return Err(Error::Conflict("version is already registered from another draft revision; choose a new semantic version".into()));
	}
	let behavioral_tested: bool = if let Some((_, _, tested)) = registered_revision {
		tested
	} else {
		let count: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("count(*)"))
				.from(Alias::new("agent_test_sessions"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("revision")).eq(Expr::cust("$2")))
						.add(Expr::col(Alias::new("status")).eq("completed")),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(draft.revision)
		.fetch_one(&mut *tx)
		.await?;
		count > 0
	};
	let inserted = crate::registry::register_in(&mut tx, &entry, &f.config.node_id).await?;
	let documents: Vec<ReferenceDocument> = serde_json::from_value(draft.documents.clone())?;
	if !documents.is_empty() {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("agent_knowledge"))
				.columns([
					Alias::new("agent_id"),
					Alias::new("agent_version"),
					Alias::new("documents"),
				])
				.values_panic([Expr::cust("$1"), Expr::cust("$2"), Expr::cust("$3")])
				.on_conflict(
					OnConflict::columns([Alias::new("agent_id"), Alias::new("agent_version")])
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&entry.id)
		.bind(&entry.version)
		.bind(&draft.documents)
		.execute(&mut *tx)
		.await?;
	}
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_draft_registrations"))
			.columns([
				Alias::new("draft_id"),
				Alias::new("revision"),
				Alias::new("agent_id"),
				Alias::new("version"),
				Alias::new("actor"),
				Alias::new("release_notes"),
				Alias::new("source_id"),
				Alias::new("source_version"),
				Alias::new("behavioral_tested"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("$5"),
				Expr::cust("$6"),
				Expr::cust("$7"),
				Expr::cust("$8"),
				Expr::cust("$9"),
			])
			.on_conflict(
				OnConflict::columns([Alias::new("draft_id"), Alias::new("revision")])
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(draft.revision)
	.bind(&entry.id)
	.bind(&entry.version)
	.bind(match &actor {
		Actor::Operator => "operator",
		Actor::Subject(identity) => &identity.subject,
	})
	.bind(&draft.release_notes)
	.bind(&draft.source_id)
	.bind(&draft.source_version)
	.bind(behavioral_tested)
	.execute(&mut *tx)
	.await?;
	if inserted {
		f.store.event(&mut tx, None, "registry.registered", json!({"id":entry.id,"version":entry.version,"kind":"agent","draft_id":id,"draft_revision":draft.revision})).await?;
	}
	tx.commit().await?;
	Ok(Json(Registration {
		draft_id: id,
		revision: draft.revision,
		entry,
		behavioral_tested,
	}))
}
