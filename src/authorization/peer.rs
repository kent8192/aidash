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
    Ok(Json(sqlx::query_as("SELECT * FROM authorization_peer_mapping_history WHERE tenant=$1 AND sequence>$2 ORDER BY sequence LIMIT $3")
        .bind(tenant).bind(page.after).bind(page.limit).fetch_all(&f.store.pool).await?))
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
    Ok(Json(sqlx::query_as("SELECT * FROM authorization_peer_mappings WHERE tenant=$1 ORDER BY source_node,source_tenant,source_subject LIMIT $2 OFFSET $3")
        .bind(tenant).bind(page.limit).bind(page.offset).fetch_all(&f.store.pool).await?))
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
        "SELECT subject FROM authorization_credentials WHERE tenant=$1 AND id=$2 FOR SHARE",
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
        sqlx::query_as("INSERT INTO authorization_peer_mappings(source_node,source_tenant,source_subject,tenant,credential_id,enabled,revision,actor) VALUES($1,$2,$3,$4,$5,$6,1,'operator') ON CONFLICT DO NOTHING RETURNING *")
            .bind(&input.source_node).bind(&input.source_tenant).bind(&input.source_subject).bind(tenant).bind(input.credential_id).bind(input.enabled).fetch_optional(&mut *tx).await?
    } else {
        sqlx::query_as("UPDATE authorization_peer_mappings SET credential_id=$5,enabled=$6,revision=revision+1,actor='operator',updated_at=clock_timestamp() WHERE source_node=$1 AND source_tenant=$2 AND source_subject=$3 AND tenant=$4 AND revision=$7 RETURNING *")
            .bind(&input.source_node).bind(&input.source_tenant).bind(&input.source_subject).bind(tenant).bind(input.credential_id).bind(input.enabled).bind(input.expected_revision).fetch_optional(&mut *tx).await?
    }.ok_or_else(|| Error::Conflict("peer mapping revision or tenant changed".into()))?;
    sqlx::query("INSERT INTO authorization_peer_mapping_history(source_node,source_tenant,source_subject,tenant,credential_id,enabled,revision,actor) VALUES($1,$2,$3,$4,$5,$6,$7,'operator')")
        .bind(&mapping.source_node).bind(&mapping.source_tenant).bind(&mapping.source_subject).bind(&mapping.tenant).bind(mapping.credential_id).bind(mapping.enabled).bind(mapping.revision).execute(&mut *tx).await?;
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
    let mapping: PeerMapping = sqlx::query_as("SELECT * FROM authorization_peer_mappings WHERE source_node=$1 AND source_tenant=$2 AND source_subject=$3 AND enabled")
        .bind(node).bind(tenant).bind(subject).fetch_optional(&f.store.pool).await?.ok_or(Error::Forbidden)?;
    let local_subject: String = sqlx::query_scalar(
        "SELECT subject FROM authorization_credentials WHERE id=$1 AND tenant=$2",
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
    let current: Option<PeerMapping> = sqlx::query_as("SELECT * FROM authorization_peer_mappings WHERE source_node=$1 AND source_tenant=$2 AND source_subject=$3 AND enabled FOR SHARE")
        .bind(node).bind(tenant).bind(subject).fetch_optional(&mut *access.tx).await?;
    if current.as_ref() != Some(&mapping) {
        return Err(Error::Forbidden);
    }
    // Retain enabled peer admission through the metadata read, just as the
    // mapped policy, credential and binding remain leased until completion.
    let enabled: Option<String> =
        sqlx::query_scalar("SELECT node_id FROM peers WHERE node_id=$1 AND enabled FOR SHARE")
            .bind(node)
            .fetch_optional(&mut *access.tx)
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
