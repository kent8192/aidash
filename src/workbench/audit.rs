//! Bounded, source-attributed factual history for an exact agent version.
use super::*;
use crate::registry::EntityRef;
use axum::extract::Query as AxumQuery;

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct AuditQuery {
	#[serde(default)]
	offset: usize,
	tenant: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditItem {
	pub source: String,
	pub kind: String,
	pub at: DateTime<Utc>,
	pub actor: Option<String>,
	pub details: Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditPage {
	pub observed_at: DateTime<Utc>,
	pub items: Vec<AuditItem>,
	pub next_offset: Option<usize>,
	pub source_boundary: String,
}

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new().routes(routes!(audit))
}

#[utoipa::path(get,path="/workbench/versions/{id}/{version}/audit",operation_id="workbench_version_audit",params(("id"=String,Path),("version"=String,Path),AuditQuery),responses((status=200,body=AuditPage)),security(("bearer_auth"=[])))]
async fn audit(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, version)): Path<(String, String)>,
	AxumQuery(query): AxumQuery<AuditQuery>,
) -> Result<Json<AuditPage>> {
	if query.offset > 10_000 {
		return Err(Error::Invalid("audit offset too large".into()));
	}
	let tenant = match &actor {
		Actor::Operator => query.tenant.clone(),
		Actor::Subject(identity) => {
			if query
				.tenant
				.as_ref()
				.is_some_and(|tenant| tenant != &identity.tenant)
			{
				return Err(Error::Forbidden);
			}
			Some(identity.tenant.clone())
		}
	};
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
	let mut items = Vec::new();
	let registrations: Vec<(Uuid, i64, String, DateTime<Utc>)> = sqlx::query_as(
		&Query::select()
			.columns([
				Alias::new("draft_id"),
				Alias::new("revision"),
				Alias::new("actor"),
				Alias::new("registered_at"),
			])
			.from(Alias::new("agent_draft_registrations"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("version")).eq(Expr::cust("$2"))),
			)
			.limit(100)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&id)
	.bind(&version)
	.fetch_all(&mut *tx)
	.await?;
	for (draft_id, revision, registering_actor, at) in registrations {
		let draft = load(&mut tx, draft_id, false).await?;
		if tenant
			.as_ref()
			.is_some_and(|tenant| tenant != &draft.tenant)
		{
			continue;
		}
		match authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await {
			Ok(()) => {}
			Err(Error::Forbidden) => continue,
			Err(error) => return Err(error),
		}
		items.push(AuditItem {
			source: "registry".into(),
			kind: "registered".into(),
			at,
			actor: Some(registering_actor),
			details: json!({"draft_id":draft_id,"draft_revision":revision}),
		});
		let sessions: Vec<(Uuid, String, DateTime<Utc>, Value)> = sqlx::query_as(
			&Query::select()
				.columns([
					Alias::new("id"),
					Alias::new("status"),
					Alias::new("created_at"),
					Alias::new("usage"),
				])
				.from(Alias::new("agent_test_sessions"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("revision")).eq(Expr::cust("$2"))),
				)
				.order_by(Alias::new("created_at"), Order::Desc)
				.limit(100)
				.to_string(PostgresQueryBuilder),
		)
		.bind(draft_id)
		.bind(revision)
		.fetch_all(&mut *tx)
		.await?;
		for (session_id, status, at, usage) in sessions {
			items.push(AuditItem { source: "sandbox".into(), kind: "test".into(), at, actor: None, details: json!({"session_id":session_id,"draft_revision":revision,"status":status,"usage":usage}) });
		}
	}
	let may_read_catalog_history = match (&actor, &tenant) {
		(Actor::Operator, _) => true,
		(Actor::Subject(identity), Some(tenant)) => {
			Authorization::evaluate_in_transaction(
				&mut tx,
				tenant,
				&Evaluation {
					subject: identity.subject.clone(),
					action: "authorization_catalog.history.read".into(),
					resource: Resource {
						tenant: tenant.clone(),
						kind: "authorization_catalog".into(),
						id: format!("{id}@{version}"),
						attributes: json!({"entry_id":id,"entry_version":version}),
					},
					environment: json!({}),
				},
			)
			.await?
			.allowed
		}
		_ => false,
	};
	if may_read_catalog_history && let Some(tenant) = &tenant {
		let history: Vec<(i64, bool, String, DateTime<Utc>)> = sqlx::query_as(
			&Query::select()
				.columns([
					Alias::new("revision"),
					Alias::new("enabled"),
					Alias::new("actor"),
					Alias::new("created_at"),
				])
				.from(Alias::new("authorization_catalog_history"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("entry_id")).eq(Expr::cust("$2")))
						.add(Expr::col(Alias::new("entry_version")).eq(Expr::cust("$3"))),
				)
				.order_by(Alias::new("created_at"), Order::Desc)
				.limit(100)
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(&id)
		.bind(&version)
		.fetch_all(&mut *tx)
		.await?;
		for (revision, enabled, actor, at) in history {
			items.push(AuditItem {
				source: "catalog".into(),
				kind: "binding_changed".into(),
				at,
				actor: Some(actor),
				details: json!({"tenant":tenant,"revision":revision,"enabled":enabled}),
			});
		}
	}
	tx.commit().await?;
	let incidents = incident::list(State(f.clone()), Extension(actor), Path((id, version)))
		.await?
		.0;
	for incident in incidents {
		let events: Vec<(String, Value, DateTime<Utc>)> = sqlx::query_as(
			&Query::select()
				.columns([
					Alias::new("actor"),
					Alias::new("change"),
					Alias::new("created_at"),
				])
				.from(Alias::new("agent_incident_events"))
				.and_where(Expr::col(Alias::new("incident_id")).eq(Expr::cust("$1")))
				.order_by(Alias::new("created_at"), Order::Desc)
				.limit(100)
				.to_string(PostgresQueryBuilder),
		)
		.bind(incident.id)
		.fetch_all(&f.store.pool)
		.await?;
		for (actor, change, at) in events {
			items.push(AuditItem {
				source: "incident".into(),
				kind: "incident_changed".into(),
				at,
				actor: Some(actor),
				details: json!({"incident_id":incident.id,"change":change}),
			});
		}
	}
	items.sort_by_key(|item| std::cmp::Reverse(item.at));
	let next_offset = (items.len() > query.offset + 50).then_some(query.offset + 50);
	let items = items.into_iter().skip(query.offset).take(50).collect();
	Ok(Json(AuditPage { observed_at: Utc::now(), items, next_offset, source_boundary: "Connected-node Registry, authorized sandbox records, tenant Catalog history and visible incident history, limited to the latest 100 records per source. Other nodes and hidden records are not represented.".into() }))
}
