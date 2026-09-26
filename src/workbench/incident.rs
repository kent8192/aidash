//! Manually reported version incidents. Severity is the reporter's label, not
//! an Aidash assessment. Evidence is a fixed copy with a separate retention clock.
use super::*;
use crate::registry::EntityRef;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceInput {
	pub title: String,
	pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvidenceCopy {
	pub title: String,
	pub content: Option<String>,
	pub sha256: String,
	pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Incident {
	pub id: Uuid,
	pub tenant: String,
	pub agent_id: String,
	pub version: String,
	pub revision: i64,
	pub severity: String,
	pub status: String,
	pub archived: bool,
	pub owner: String,
	pub notes: String,
	pub evidence: Value,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub resolved_at: Option<DateTime<Utc>>,
	pub evidence_expires_at: Option<DateTime<Utc>>,
	pub evidence_expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct IncidentEvent {
	pub id: i64,
	pub incident_id: Uuid,
	pub actor: String,
	pub change: Value,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateIncident {
	pub tenant: Option<String>,
	pub severity: String,
	pub owner: String,
	pub notes: String,
	#[serde(default)]
	pub evidence: Vec<EvidenceInput>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateIncident {
	pub expected_revision: i64,
	pub severity: String,
	pub status: String,
	pub archived: Option<bool>,
	pub owner: String,
	pub notes: String,
	#[serde(default)]
	pub add_evidence: Vec<EvidenceInput>,
}

const INCIDENT_COLUMNS: &str = "id, tenant, agent_id, version, revision, severity, status, archived, owner, notes, evidence, created_at, updated_at, resolved_at, evidence_expires_at, evidence_expired_at";

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(list))
		.routes(routes!(create))
		.routes(routes!(get))
		.routes(routes!(update))
		.routes(routes!(events))
}

fn validate(severity: &str, status: &str, notes: &str, evidence: &[EvidenceInput]) -> Result<()> {
	if !matches!(severity, "low" | "medium" | "high" | "critical")
		|| !matches!(status, "open" | "resolved")
		|| notes.trim().is_empty()
		|| notes.len() > 16_384
		|| evidence.len() > 8
		|| evidence
			.iter()
			.map(|item| item.content.len())
			.sum::<usize>()
			> 65_536
		|| evidence.iter().any(|item| {
			item.title.trim().is_empty() || item.title.len() > 255 || item.content.trim().is_empty()
		}) {
		return Err(Error::Invalid(
			"invalid incident severity, status, notes or evidence".into(),
		));
	}
	Ok(())
}

fn fixed_copies(evidence: Vec<EvidenceInput>) -> Vec<EvidenceCopy> {
	evidence
		.into_iter()
		.map(|item| EvidenceCopy {
			sha256: format!("{:x}", Sha256::digest(item.content.as_bytes())),
			title: item.title,
			content: Some(item.content),
			recorded_at: Utc::now(),
		})
		.collect()
}

fn actor_name(actor: &Actor) -> &str {
	match actor {
		Actor::Operator => "operator",
		Actor::Subject(identity) => &identity.subject,
	}
}

async fn require_incident(
	tx: &mut Transaction<'_, Postgres>,
	actor: &Actor,
	incident: &Incident,
	action: &str,
) -> Result<()> {
	let Actor::Subject(identity) = actor else {
		return Ok(());
	};
	if identity.tenant != incident.tenant {
		return Err(Error::Forbidden);
	}
	let decision = Authorization::evaluate_in_transaction(tx, &incident.tenant, &Evaluation {
		subject: identity.subject.clone(), action: action.into(),
		resource: Resource { tenant: incident.tenant.clone(), kind: "agent_incident".into(), id: incident.id.to_string(), attributes: json!({"agent_id":incident.agent_id,"version":incident.version,"owner":incident.owner,"status":incident.status,"archived":incident.archived}) },
		environment: json!({}),
	}).await?;
	if decision.allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

async fn load(tx: &mut Transaction<'_, Postgres>, id: Uuid, lock: bool) -> Result<Incident> {
	let mut query = Query::select();
	query
		.expr(Expr::cust(INCIDENT_COLUMNS))
		.from(Alias::new("agent_incidents"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")));
	if lock {
		query.lock(sea_orm::sea_query::LockType::Update);
	}
	sqlx::query_as(&query.to_string(PostgresQueryBuilder))
		.bind(id)
		.fetch_optional(&mut **tx)
		.await?
		.ok_or_else(|| Error::NotFound("incident".into()))
}

async fn record(
	tx: &mut Transaction<'_, Postgres>,
	incident: Uuid,
	actor: &str,
	change: Value,
) -> Result<()> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_incident_events"))
			.columns([
				Alias::new("incident_id"),
				Alias::new("actor"),
				Alias::new("change"),
			])
			.values_panic([Expr::cust("$1"), Expr::cust("$2"), Expr::cust("$3")])
			.to_string(PostgresQueryBuilder),
	)
	.bind(incident)
	.bind(actor)
	.bind(change)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

#[utoipa::path(post,path="/workbench/versions/{id}/{version}/incidents",operation_id="workbench_create_incident",params(("id"=String,Path),("version"=String,Path)),request_body=CreateIncident,responses((status=200,body=Incident)),security(("bearer_auth"=[])))]
async fn create(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, version)): Path<(String, String)>,
	Json(input): Json<CreateIncident>,
) -> Result<Json<Incident>> {
	validate(&input.severity, "open", &input.notes, &input.evidence)?;
	let (tenant, _) = author_identity(&actor, input.tenant.as_deref(), Some(actor_name(&actor)))?;
	target_enabled(&f, &tenant, &input.owner).await?;
	let mut tx = f.store.pool.begin().await?;
	let reference = EntityRef { id, version };
	trust::require_inspection(&mut tx, &actor, &reference).await?;
	if f.registry
		.get(&reference.id, &reference.version)
		.await?
		.kind != "agent"
	{
		return Err(Error::NotFound("agent version".into()));
	}
	let incident_id = Uuid::now_v7();
	let candidate = Incident {
		id: incident_id,
		tenant,
		agent_id: reference.id,
		version: reference.version,
		revision: 1,
		severity: input.severity,
		status: "open".into(),
		archived: false,
		owner: input.owner,
		notes: input.notes,
		evidence: serde_json::to_value(fixed_copies(input.evidence))?,
		created_at: Utc::now(),
		updated_at: Utc::now(),
		resolved_at: None,
		evidence_expires_at: None,
		evidence_expired_at: None,
	};
	require_incident(&mut tx, &actor, &candidate, "agent_incident.manage").await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_incidents"))
			.columns([
				Alias::new("id"),
				Alias::new("tenant"),
				Alias::new("agent_id"),
				Alias::new("version"),
				Alias::new("severity"),
				Alias::new("owner"),
				Alias::new("notes"),
				Alias::new("evidence"),
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
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(incident_id)
	.bind(&candidate.tenant)
	.bind(&candidate.agent_id)
	.bind(&candidate.version)
	.bind(&candidate.severity)
	.bind(&candidate.owner)
	.bind(&candidate.notes)
	.bind(&candidate.evidence)
	.execute(&mut *tx)
	.await?;
	record(&mut tx, incident_id, actor_name(&actor), json!({"created":true,"severity":candidate.severity,"status":"open","evidence_count":candidate.evidence.as_array().map_or(0,Vec::len)})).await?;
	let result = load(&mut tx, incident_id, false).await?;
	tx.commit().await?;
	Ok(Json(result))
}

#[utoipa::path(get,path="/workbench/versions/{id}/{version}/incidents",operation_id="workbench_list_incidents",params(("id"=String,Path),("version"=String,Path)),responses((status=200,body=[Incident])),security(("bearer_auth"=[])))]
pub(super) async fn list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, version)): Path<(String, String)>,
) -> Result<Json<Vec<Incident>>> {
	let mut tx = f.store.pool.begin().await?;
	trust::require_inspection(
		&mut tx,
		&actor,
		&EntityRef {
			id: id.clone(),
			version: version.clone(),
		},
	)
	.await?;
	let mut visible = Vec::new();
	let mut cursor: Option<(DateTime<Utc>, Uuid)> = None;
	let tenant = match &actor {
		Actor::Operator => None,
		Actor::Subject(identity) => Some(&identity.tenant),
	};
	loop {
		let mut query = Query::select();
		query
			.expr(Expr::cust(INCIDENT_COLUMNS))
			.from(Alias::new("agent_incidents"))
			.and_where(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$2")))
			.cond_where(
				Condition::any()
					.add(Expr::expr(Expr::cust("$3::text")).is_null())
					.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$3"))),
			)
			.order_by(Alias::new("created_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(100);
		if cursor.is_some() {
			query.cond_where(
				Condition::any()
					.add(Expr::col(Alias::new("created_at")).lt(Expr::cust("$4")))
					.add(
						Condition::all()
							.add(Expr::col(Alias::new("created_at")).eq(Expr::cust("$4")))
							.add(Expr::col(Alias::new("id")).lt(Expr::cust("$5"))),
					),
			);
		}
		let sql = query.to_string(PostgresQueryBuilder);
		let mut select = sqlx::query_as::<_, Incident>(&sql)
			.bind(&id)
			.bind(&version)
			.bind(tenant);
		if let Some((created_at, incident_id)) = cursor {
			select = select.bind(created_at).bind(incident_id);
		}
		let rows = select.fetch_all(&mut *tx).await?;
		let more = rows.len() == 100;
		cursor = rows.last().map(|row| (row.created_at, row.id));
		for row in rows {
			match require_incident(&mut tx, &actor, &row, "agent_incident.read").await {
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

#[utoipa::path(get,path="/workbench/incidents/{id}",operation_id="workbench_get_incident",params(("id"=Uuid,Path)),responses((status=200,body=Incident)),security(("bearer_auth"=[])))]
async fn get(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Incident>> {
	let mut tx = f.store.pool.begin().await?;
	let incident = load(&mut tx, id, false).await?;
	trust::require_inspection(
		&mut tx,
		&actor,
		&EntityRef {
			id: incident.agent_id.clone(),
			version: incident.version.clone(),
		},
	)
	.await?;
	require_incident(&mut tx, &actor, &incident, "agent_incident.read").await?;
	tx.commit().await?;
	Ok(Json(incident))
}

#[utoipa::path(put,path="/workbench/incidents/{id}",operation_id="workbench_update_incident",params(("id"=Uuid,Path)),request_body=UpdateIncident,responses((status=200,body=Incident)),security(("bearer_auth"=[])))]
async fn update(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<UpdateIncident>,
) -> Result<Json<Incident>> {
	validate(
		&input.severity,
		&input.status,
		&input.notes,
		&input.add_evidence,
	)?;
	let mut tx = f.store.pool.begin().await?;
	let prior = load(&mut tx, id, true).await?;
	trust::require_inspection(
		&mut tx,
		&actor,
		&EntityRef {
			id: prior.agent_id.clone(),
			version: prior.version.clone(),
		},
	)
	.await?;
	require_incident(&mut tx, &actor, &prior, "agent_incident.manage").await?;
	if prior.revision != input.expected_revision {
		return Err(Error::Conflict("incident revision changed".into()));
	}
	target_enabled(&f, &prior.tenant, &input.owner).await?;
	let mut evidence: Vec<EvidenceCopy> = serde_json::from_value(prior.evidence.clone())?;
	if prior.evidence_expired_at.is_some() && !input.add_evidence.is_empty() {
		return Err(Error::Conflict(
			"expired evidence cannot be revived; open a new incident".into(),
		));
	}
	evidence.extend(fixed_copies(input.add_evidence));
	if evidence.len() > 32 {
		return Err(Error::Invalid("incident evidence exceeds 32 copies".into()));
	}
	let was_resolved = prior.status == "resolved";
	let resolved = input.status == "resolved";
	let (resolved_at, expires_at) = match (was_resolved, resolved) {
		(false, true) => {
			let days = test::limits(&mut tx, &prior.tenant)
				.await?
				.incident_evidence_days;
			let now = Utc::now();
			(
				Some(now),
				Some(now + chrono::Duration::days(i64::from(days))),
			)
		}
		(true, true) => (prior.resolved_at, prior.evidence_expires_at),
		_ => (None, None),
	};
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_incidents"))
			.value(Alias::new("revision"), Expr::cust("revision + 1"))
			.value(Alias::new("severity"), Expr::cust("$2"))
			.value(Alias::new("status"), Expr::cust("$3"))
			.value(Alias::new("archived"), Expr::cust("$9"))
			.value(Alias::new("owner"), Expr::cust("$4"))
			.value(Alias::new("notes"), Expr::cust("$5"))
			.value(Alias::new("evidence"), Expr::cust("$6"))
			.value(Alias::new("resolved_at"), Expr::cust("$7"))
			.value(Alias::new("evidence_expires_at"), Expr::cust("$8"))
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&input.severity)
	.bind(&input.status)
	.bind(&input.owner)
	.bind(&input.notes)
	.bind(serde_json::to_value(&evidence)?)
	.bind(resolved_at)
	.bind(expires_at)
	.bind(input.archived.unwrap_or(prior.archived))
	.execute(&mut *tx)
	.await?;
	record(&mut tx, id, actor_name(&actor), json!({"from_revision":prior.revision,"severity":input.severity,"status":input.status,"archived":input.archived.unwrap_or(prior.archived),"owner":input.owner,"evidence_count":evidence.len()})).await?;
	let result = load(&mut tx, id, false).await?;
	tx.commit().await?;
	Ok(Json(result))
}

#[utoipa::path(get,path="/workbench/incidents/{id}/events",operation_id="workbench_incident_events",params(("id"=Uuid,Path)),responses((status=200,body=[IncidentEvent])),security(("bearer_auth"=[])))]
async fn events(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Vec<IncidentEvent>>> {
	let mut tx = f.store.pool.begin().await?;
	let incident = load(&mut tx, id, false).await?;
	trust::require_inspection(
		&mut tx,
		&actor,
		&EntityRef {
			id: incident.agent_id.clone(),
			version: incident.version.clone(),
		},
	)
	.await?;
	require_incident(&mut tx, &actor, &incident, "agent_incident.read").await?;
	let events = sqlx::query_as(
		&Query::select()
			.expr(Expr::cust("id, incident_id, actor, change, created_at"))
			.from(Alias::new("agent_incident_events"))
			.and_where(Expr::col(Alias::new("incident_id")).eq(Expr::cust("$1")))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(500)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_all(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(Json(events))
}

/// Expire copied payloads while keeping source, digest, actor and status data.
pub async fn purge_expired(pool: &sqlx::PgPool) -> Result<u64> {
	let mut count = 0;
	loop {
		let rows: Vec<Incident> = sqlx::query_as(
			&Query::select()
				.expr(Expr::cust(INCIDENT_COLUMNS))
				.from(Alias::new("agent_incidents"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("status")).eq("resolved"))
						.add(
							Expr::col(Alias::new("evidence_expires_at"))
								.lte(Expr::current_timestamp()),
						)
						.add(Expr::col(Alias::new("evidence_expired_at")).is_null()),
				)
				.limit(100)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(pool)
		.await?;
		if rows.is_empty() {
			break;
		}
		for row in rows {
			let mut tx = pool.begin().await?;
			let current = load(&mut tx, row.id, true).await?;
			if current.status != "resolved"
				|| current.evidence_expired_at.is_some()
				|| current.evidence_expires_at.is_none_or(|at| at > Utc::now())
			{
				tx.commit().await?;
				continue;
			}
			let mut copies: Vec<EvidenceCopy> = serde_json::from_value(current.evidence)?;
			for copy in &mut copies {
				copy.content = None;
			}
			sqlx::query(
				&Query::update()
					.table(Alias::new("agent_incidents"))
					.value(Alias::new("evidence"), Expr::cust("$2"))
					.value(Alias::new("evidence_expired_at"), Expr::current_timestamp())
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.to_string(PostgresQueryBuilder),
			)
			.bind(row.id)
			.bind(serde_json::to_value(copies)?)
			.execute(&mut *tx)
			.await?;
			tx.commit().await?;
			count += 1;
		}
	}
	Ok(count)
}
