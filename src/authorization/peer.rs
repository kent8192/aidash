//! Explicit inbound identity mappings. Peer authentication alone grants no
//! tenant authority, and local subject bearer tokens never cross this boundary.
pub(crate) mod admission;
pub(crate) mod discovery;
pub(crate) mod execution;
pub(crate) mod reads;

use super::{
	Authorization, access::Access, catalog, identity::SubjectIdentity, policy::identifier,
};
use crate::{
	Error, Result,
	federation::Federation,
	registry::{Entry, Search},
};
use axum::{
	Json,
	extract::{Path, Query, State},
	http::HeaderMap,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct PeerMapping {
	pub source_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub tenant: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub revision: i64,
	pub actor: String,
	pub updated_at: DateTime<Utc>,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerMappingInput {
	pub source_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub expected_revision: i64,
}

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(list, set))
		.routes(routes!(history))
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct HistoryPage {
	#[serde(default)]
	after: i64,
	#[serde(default = "page_size")]
	limit: i64,
}
fn page_size() -> i64 {
	100
}
#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
struct MappingRevision {
	sequence: i64,
	#[serde(flatten)]
	#[sqlx(flatten)]
	mapping: PeerMapping,
}
#[utoipa::path(get,path="/authorization/{tenant}/peer-mapping-history",operation_id="authorization_peer_mapping_history",params(("tenant"=String,Path),HistoryPage),responses((status=200,body=[MappingRevision])),security(("bearer_auth"=[])))]
async fn history(
	State(f): State<Federation>,
	Path(tenant): Path<String>,
	Query(page): Query<HistoryPage>,
) -> Result<Json<Vec<MappingRevision>>> {
	identifier(&tenant)?;
	if page.after < 0 || !(1..=200).contains(&page.limit) {
		return Err(Error::Invalid("invalid history page".into()));
	}
	Ok(Json(
		sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new(
					"authorization_peer_mapping_history",
				))
				.and_where(sea_orm::sea_query::Expr::cust(
					"tenant = $1 AND sequence > $2",
				))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("sequence"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.limit(page.limit as u64)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(page.after)
		.fetch_all(&f.store.pool)
		.await?,
	))
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct MappingPage {
	#[serde(default)]
	offset: i64,
	#[serde(default = "page_size")]
	limit: i64,
}
#[utoipa::path(get,path="/authorization/{tenant}/peer-mappings",operation_id="authorization_peer_mappings",params(("tenant"=String,Path),MappingPage),responses((status=200,body=[PeerMapping])),security(("bearer_auth"=[])))]
async fn list(
	State(f): State<Federation>,
	Path(tenant): Path<String>,
	Query(page): Query<MappingPage>,
) -> Result<Json<Vec<PeerMapping>>> {
	identifier(&tenant)?;
	if page.offset < 0 || !(1..=200).contains(&page.limit) {
		return Err(Error::Invalid("invalid mapping page".into()));
	}
	Ok(Json(
		sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new(
					"authorization_peer_mappings",
				))
				.and_where(sea_orm::sea_query::Expr::cust("tenant = $1"))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("source_node"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("source_tenant"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("source_subject"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.limit(page.limit as u64)
				.offset(page.offset as u64)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(tenant)
		.fetch_all(&f.store.pool)
		.await?,
	))
}
#[utoipa::path(post,path="/authorization/{tenant}/peer-mappings",operation_id="authorization_set_peer_mapping",params(("tenant"=String,Path)),request_body=PeerMappingInput,responses((status=200,body=PeerMapping)),security(("bearer_auth"=[])))]
async fn set(
	State(f): State<Federation>,
	Path(tenant): Path<String>,
	Json(input): Json<PeerMappingInput>,
) -> Result<Json<PeerMapping>> {
	Ok(Json(write(&f, &tenant, input).await?))
}
pub async fn write(f: &Federation, tenant: &str, input: PeerMappingInput) -> Result<PeerMapping> {
	identifier(tenant)?;
	identifier(&input.source_tenant)?;
	identifier(&input.source_subject)?;
	crate::config::validate_node_id(&input.source_node)?;
	if input.source_node == f.config.node_id || !(0..i64::MAX).contains(&input.expected_revision) {
		return Err(Error::Invalid("invalid peer mapping or revision".into()));
	}
	if input.enabled {
		f.peer(&input.source_node).await?;
	}
	let mut tx = f.store.pool.begin().await?;
	Authorization::load(&mut tx, tenant).await?;
	let subject: Option<String> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("subject")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_credentials"))
			.and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND id = $2"))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(tenant)
	.bind(input.credential_id)
	.fetch_optional(&mut *tx)
	.await?;
	let identity = SubjectIdentity {
		credential_id: input.credential_id,
		tenant: tenant.into(),
		subject: subject.ok_or(Error::Forbidden)?,
	};
	if input.enabled {
		identity
			.lock_with_mode(&mut tx, false)
			.await
			.map_err(mapping_authority_error)?;
	}
	let mapping: PeerMapping = if input.expected_revision == 0 {
        sqlx::query_as(&sea_orm::sea_query::Query::insert().into_table(sea_orm::sea_query::Alias::new("authorization_peer_mappings")).columns([sea_orm::sea_query::Alias::new("source_node"), sea_orm::sea_query::Alias::new("source_tenant"), sea_orm::sea_query::Alias::new("source_subject"), sea_orm::sea_query::Alias::new("tenant"), sea_orm::sea_query::Alias::new("credential_id"), sea_orm::sea_query::Alias::new("enabled"), sea_orm::sea_query::Alias::new("revision"), sea_orm::sea_query::Alias::new("actor")]).values_panic([sea_orm::sea_query::Expr::cust("$1"), sea_orm::sea_query::Expr::cust("$2"), sea_orm::sea_query::Expr::cust("$3"), sea_orm::sea_query::Expr::cust("$4"), sea_orm::sea_query::Expr::cust("$5"), sea_orm::sea_query::Expr::cust("$6"), sea_orm::sea_query::Expr::cust("1"), sea_orm::sea_query::Expr::cust("'operator'")]).on_conflict(sea_orm::sea_query::OnConflict::new().do_nothing().to_owned()).returning_all().to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(&input.source_node).bind(&input.source_tenant).bind(&input.source_subject).bind(tenant).bind(input.credential_id).bind(input.enabled).fetch_optional(&mut *tx).await?
    } else {
        sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("authorization_peer_mappings")).value(sea_orm::sea_query::Alias::new("credential_id"), sea_orm::sea_query::Expr::cust("$5")).value(sea_orm::sea_query::Alias::new("enabled"), sea_orm::sea_query::Expr::cust("$6")).value(sea_orm::sea_query::Alias::new("revision"), sea_orm::sea_query::Expr::cust("revision + 1")).value(sea_orm::sea_query::Alias::new("actor"), sea_orm::sea_query::Expr::cust("'operator'")).value(sea_orm::sea_query::Alias::new("updated_at"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()")).and_where(sea_orm::sea_query::Expr::cust("source_node = $1 AND source_tenant = $2 AND source_subject = $3 AND tenant = $4 AND revision = $7")).returning_all().to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(&input.source_node).bind(&input.source_tenant).bind(&input.source_subject).bind(tenant).bind(input.credential_id).bind(input.enabled).bind(input.expected_revision).fetch_optional(&mut *tx).await?
    }.ok_or_else(|| Error::Conflict("peer mapping revision or tenant changed".into()))?;
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new(
				"authorization_peer_mapping_history",
			))
			.columns([
				sea_orm::sea_query::Alias::new("source_node"),
				sea_orm::sea_query::Alias::new("source_tenant"),
				sea_orm::sea_query::Alias::new("source_subject"),
				sea_orm::sea_query::Alias::new("tenant"),
				sea_orm::sea_query::Alias::new("credential_id"),
				sea_orm::sea_query::Alias::new("enabled"),
				sea_orm::sea_query::Alias::new("revision"),
				sea_orm::sea_query::Alias::new("actor"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
				sea_orm::sea_query::Expr::cust("$5"),
				sea_orm::sea_query::Expr::cust("$6"),
				sea_orm::sea_query::Expr::cust("$7"),
				sea_orm::sea_query::Expr::cust("'operator'"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&mapping.source_node)
	.bind(&mapping.source_tenant)
	.bind(&mapping.source_subject)
	.bind(&mapping.tenant)
	.bind(mapping.credential_id)
	.bind(mapping.enabled)
	.bind(mapping.revision)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(mapping)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiscoveryInput {
	tenant: String,
	subject: String,
	search: Search,
}
// The peer/operator is already authenticated. An unusable mapped credential
// denies authority; it does not challenge the caller to replace its bearer.
fn mapping_authority_error(error: Error) -> Error {
	match error {
		Error::Unauthorized => Error::Forbidden,
		other => other,
	}
}
async fn access(f: &Federation, node: &str, tenant: &str, subject: &str) -> Result<Access> {
	identifier(tenant)?;
	identifier(subject)?;
	let mapping: PeerMapping = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new(
				"authorization_peer_mappings",
			))
			.and_where(sea_orm::sea_query::Expr::cust(
				"source_node = $1 AND source_tenant = $2 AND source_subject = $3 AND enabled",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(node)
	.bind(tenant)
	.bind(subject)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or(Error::Forbidden)?;
	let local_subject: String = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("subject")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_credentials"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND tenant = $2"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(mapping.credential_id)
	.bind(&mapping.tenant)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or(Error::Forbidden)?;
	let mut access = Access::begin(
		&f.store,
		&SubjectIdentity {
			credential_id: mapping.credential_id,
			tenant: mapping.tenant.clone(),
			subject: local_subject,
		},
	)
	.await
	.map_err(mapping_authority_error)?;
	// Lock in the same order as management: policy, credential, then mapping.
	// A binding changed between resolution and this lease cannot select a new
	// credential or tenant under the old identity.
	let current: Option<PeerMapping> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new(
				"authorization_peer_mappings",
			))
			.and_where(sea_orm::sea_query::Expr::cust(
				"source_node = $1 AND source_tenant = $2 AND source_subject = $3 AND enabled",
			))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(node)
	.bind(tenant)
	.bind(subject)
	.fetch_optional(&mut **access.tx)
	.await?;
	if current.as_ref() != Some(&mapping) {
		return Err(Error::Forbidden);
	}
	// Retain enabled peer admission through the metadata read, just as the
	// mapped policy, credential and binding remain leased until completion.
	let enabled: Option<String> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id")),
			))
			.from(sea_orm::sea_query::Alias::new("peers"))
			.and_where(sea_orm::sea_query::Expr::cust("node_id = $1 AND enabled"))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(node)
	.fetch_optional(&mut **access.tx)
	.await?;
	if enabled.is_none() {
		return Err(Error::Forbidden);
	}
	access.environment["transport"] = json!("federation");
	access.environment["source_node"] = json!(node);
	access.context = json!({"source_node":node,"source_tenant":tenant,"source_subject":subject});
	Ok(access)
}
pub(crate) async fn discover(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<DiscoveryInput>,
) -> Result<Json<Vec<Entry>>> {
	let node = crate::api::peer_node(&headers)?;
	let mut access = access(&f, node, &input.tenant, &input.subject).await?;
	let result = async {
		let resource = access.resource("node", &f.config.node_id, json!({}));
		access.require(&resource, "federation.discover").await?;
		let mut search = input.search;
		search.kind = Some("agent".into());
		let mut visible = vec![];
		for entry in catalog::list_in(&mut access, &search).await? {
			if access
				.decide(&catalog::resource(&access, &entry), "agent.execute")
				.await?
			{
				visible.push(entry);
			}
		}
		Ok(Json(visible))
	}
	.await;
	access.finish(result).await
}

// Authority RPCs distinguish a permanent denial from a transport outage while
// never reflecting a peer's response body or internal error details.
pub(crate) async fn authority_request<T: serde::de::DeserializeOwned>(
	f: &Federation,
	node: &str,
	path: &str,
	body: &serde_json::Value,
) -> Result<T> {
	let response = f
		.peer_response(node, reqwest::Method::POST, path, Some(body))
		.await
		.map_err(|_| Error::External("remote execution authority unavailable".into()))?;
	match response.status().as_u16() {
		200..=299 => crate::response::json(response, 4_194_304)
			.await
			.map_err(|_| Error::External("invalid remote authority response".into())),
		401 | 403 | 404 => Err(Error::Forbidden),
		409 => Err(Error::Conflict("remote execution authority changed".into())),
		503 if response
			.headers()
			.get("x-aidash-transaction-pending")
			.is_some_and(|value| value == "1") =>
		{
			Err(Error::TransactionPending)
		}
		_ => Err(Error::External(
			"remote execution authority unavailable".into(),
		)),
	}
}
