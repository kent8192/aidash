//! Bounded, source-attributed factual history for an exact agent version.
use super::*;
use crate::registry::EntityRef;
use axum::extract::Query as AxumQuery;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct AuditQuery {
	cursor: Option<String>,
	tenant: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditItem {
	pub id: String,
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
	pub next_cursor: Option<String>,
	pub source_boundary: String,
}

#[derive(Serialize, Deserialize)]
struct AuditCursor {
	observed_at: DateTime<Utc>,
	before_at: DateTime<Utc>,
	before_id: String,
}

fn decode_cursor(value: &str) -> Result<AuditCursor> {
	if value.len() > 1024 {
		return Err(Error::Invalid("audit cursor too large".into()));
	}
	let bytes = URL_SAFE_NO_PAD
		.decode(value)
		.map_err(|_| Error::Invalid("invalid audit cursor".into()))?;
	let cursor: AuditCursor = serde_json::from_slice(&bytes)
		.map_err(|_| Error::Invalid("invalid audit cursor".into()))?;
	if cursor.before_at > cursor.observed_at
		|| cursor.before_id.is_empty()
		|| cursor.before_id.len() > 200
	{
		return Err(Error::Invalid("invalid audit cursor boundary".into()));
	}
	Ok(cursor)
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
	let cursor = query.cursor.as_deref().map(decode_cursor).transpose()?;
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
	let observed_at = match &cursor {
		Some(cursor) => cursor.observed_at,
		None => {
			sqlx::query_scalar(
				&Query::select()
					.expr(Expr::current_timestamp())
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await?
		}
	};
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
					.add(Expr::col(Alias::new("version")).eq(Expr::cust("$2")))
					.add(Expr::col(Alias::new("registered_at")).lte(Expr::cust("$3"))),
			)
			.order_by(Alias::new("registered_at"), Order::Desc)
			.order_by(Alias::new("draft_id"), Order::Desc)
			.order_by(Alias::new("revision"), Order::Desc)
			.limit(100)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&id)
	.bind(&version)
	.bind(observed_at)
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
			id: format!("registry:{draft_id}:{revision:020}"),
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
						.add(Expr::col(Alias::new("revision")).eq(Expr::cust("$2")))
						.add(Expr::col(Alias::new("created_at")).lte(Expr::cust("$3"))),
				)
				.order_by(Alias::new("created_at"), Order::Desc)
				.order_by(Alias::new("id"), Order::Desc)
				.limit(100)
				.to_string(PostgresQueryBuilder),
		)
		.bind(draft_id)
		.bind(revision)
		.bind(observed_at)
		.fetch_all(&mut *tx)
		.await?;
		for (session_id, status, at, usage) in sessions {
			items.push(AuditItem { id: format!("sandbox:{session_id}"), source: "sandbox".into(), kind: "test".into(), at, actor: None, details: json!({"session_id":session_id,"draft_revision":revision,"status":status,"usage":usage}) });
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
						.add(Expr::col(Alias::new("entry_version")).eq(Expr::cust("$3")))
						.add(Expr::col(Alias::new("created_at")).lte(Expr::cust("$4"))),
				)
				.order_by(Alias::new("created_at"), Order::Desc)
				.order_by(Alias::new("revision"), Order::Desc)
				.limit(100)
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(&id)
		.bind(&version)
		.bind(observed_at)
		.fetch_all(&mut *tx)
		.await?;
		for (revision, enabled, actor, at) in history {
			items.push(AuditItem {
				id: format!("catalog:{tenant}:{revision:020}"),
				source: "catalog".into(),
				kind: "binding_changed".into(),
				at,
				actor: Some(actor),
				details: json!({"tenant":tenant,"revision":revision,"enabled":enabled}),
			});
		}
	}
	tx.commit().await?;
	let incidents = incident::collect(
		&f,
		&actor,
		&id,
		&version,
		tenant.as_deref(),
		Some(observed_at),
	)
	.await?;
	for incident in incidents {
		let events =
			match incident::read_events(&f, &actor, incident.id, 100, Some(observed_at)).await {
				Ok(events) => events,
				Err(Error::Forbidden) => continue,
				Err(error) => return Err(error),
			};
		for event in events {
			items.push(AuditItem {
				id: format!("incident:{:020}", event.id),
				source: "incident".into(),
				kind: "incident_changed".into(),
				at: event.created_at,
				actor: Some(event.actor),
				details: json!({"incident_id":incident.id,"change":event.change}),
			});
		}
	}
	items.sort_by(|left, right| right.at.cmp(&left.at).then_with(|| right.id.cmp(&left.id)));
	if let Some(cursor) = &cursor {
		items.retain(|item| {
			item.at < cursor.before_at
				|| (item.at == cursor.before_at && item.id < cursor.before_id)
		});
	}
	let more = items.len() > 50;
	items.truncate(50);
	let next_cursor = if more {
		items
			.last()
			.map(|item| {
				serde_json::to_vec(&AuditCursor {
					observed_at,
					before_at: item.at,
					before_id: item.id.clone(),
				})
				.map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
			})
			.transpose()?
	} else {
		None
	};
	Ok(Json(AuditPage { observed_at, items, next_cursor, source_boundary: "Connected-node Registry, authorized sandbox records, tenant Catalog history and visible incident history, bounded by the initial observation time and the latest 100 records per source. Authorization is checked again on every page; other nodes and hidden records are not represented.".into() }))
}
