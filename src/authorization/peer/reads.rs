//! Durable remote metadata dependencies and live checks on subsequent delivery.
use super::super::{access::Access, catalog, policy::identifier};
use crate::{
	Error, Result,
	federation::{DiscoveredAgent, Federation, Peer},
	registry::{EntityRef, Entry, digest},
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reference {
	entry: EntityRef,
	digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyInput {
	tenant: String,
	subject: String,
	references: Vec<Reference>,
}
pub(crate) async fn verify(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<VerifyInput>,
) -> Result<Json<bool>> {
	if input.references.is_empty() || input.references.len() > 128 {
		return Err(Error::Invalid(
			"verification requires 1..128 references".into(),
		));
	}
	for reference in &input.references {
		identifier(&reference.entry.id)?;
		if semver::Version::parse(&reference.entry.version).is_err()
			|| !reference.digest.starts_with("sha256:")
			|| reference.digest.len() != 71
		{
			return Err(Error::Invalid("invalid remote registry reference".into()));
		}
	}
	let node = crate::api::peer_node(&headers)?;
	let mut access = super::access(&f, node, &input.tenant, &input.subject).await?;
	let result = async {
		let resource = access.resource("node", &f.config.node_id, json!({}));
		access.require(&resource, "federation.discover").await?;
		for reference in input.references {
			let entry = match catalog::entry(&mut access, &reference.entry, "registry.read").await {
				Ok(entry) => entry,
				Err(Error::Forbidden | Error::NotFound(_)) => return Ok(Json(false)),
				Err(error) => return Err(error),
			};
			if entry.kind != "agent"
				|| digest(&serde_json::to_value(&entry)?) != reference.digest
				|| !access
					.decide(&catalog::resource(&access, &entry), "agent.execute")
					.await?
			{
				return Ok(Json(false));
			}
		}
		Ok(Json(true))
	}
	.await;
	access.finish(result).await
}

impl Access {
	pub(crate) async fn track_discovery(&mut self, agents: &[DiscoveredAgent]) -> Result<()> {
		let Some(run) = self.read_run else {
			return Ok(());
		};
		let local: Vec<_> = agents
			.iter()
			.filter(|agent| agent.node_id == self.node_id)
			.map(|agent| agent.entity.clone())
			.collect();
		self.track_registry(&local).await?;
		// This independent commit precedes invocation/context persistence, so a
		// killed worker cannot retain metadata without its visibility dependency.
		let mut tx = self.pool.begin().await?;
		for agent in agents.iter().filter(|agent| agent.node_id != self.node_id) {
			let metadata = serde_json::to_value(&agent.entity)?;
			sqlx::query(
				&sea_orm::sea_query::Query::insert()
					.into_table(sea_orm::sea_query::Alias::new(
						"authorization_run_remote_reads",
					))
					.columns([
						sea_orm::sea_query::Alias::new("run_id"),
						sea_orm::sea_query::Alias::new("node_id"),
						sea_orm::sea_query::Alias::new("entry_id"),
						sea_orm::sea_query::Alias::new("entry_version"),
						sea_orm::sea_query::Alias::new("digest"),
						sea_orm::sea_query::Alias::new("metadata"),
					])
					.values_panic([
						sea_orm::sea_query::Expr::cust("$1"),
						sea_orm::sea_query::Expr::cust("$2"),
						sea_orm::sea_query::Expr::cust("$3"),
						sea_orm::sea_query::Expr::cust("$4"),
						sea_orm::sea_query::Expr::cust("$5"),
						sea_orm::sea_query::Expr::cust("$6"),
					])
					.on_conflict(
						sea_orm::sea_query::OnConflict::new()
							.do_nothing()
							.to_owned(),
					)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(run)
			.bind(&agent.node_id)
			.bind(&agent.entity.id)
			.bind(&agent.entity.version)
			.bind(digest(&metadata))
			.bind(metadata)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
		Ok(())
	}

	pub(crate) async fn remote_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let key = (run, self.authority_context());
		if let Some(allowed) = self.remote_read_cache.get(&key) {
			return Ok(*allowed);
		}
		let allowed = self.remote_reads_visible_in(run).await?;
		self.remote_read_cache.insert(key, allowed);
		Ok(allowed)
	}
	async fn remote_reads_visible_in(&mut self, run: Uuid) -> Result<bool> {
		let rows: Vec<(String, String, String, String, Value)> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("entry_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("entry_version")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("digest")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("metadata")),
				))
				.from(sea_orm::sea_query::Alias::new(
					"authorization_run_remote_reads",
				))
				.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("node_id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("entry_id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("entry_version"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("digest"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run)
		.fetch_all(&mut **self.tx)
		.await?;
		let mut nodes: BTreeMap<String, Vec<Reference>> = BTreeMap::new();
		for (node, id, version, hash, metadata) in rows {
			let entry: Entry = serde_json::from_value(metadata.clone())?;
			if entry.id != id
				|| entry.version != version
				|| entry.kind != "agent"
				|| digest(&metadata) != hash
			{
				return Ok(false);
			}
			let mut resource = catalog::resource(self, &entry);
			resource.id = crate::domain::qualified_agent(&node, &id, &version);
			resource.attributes["remote_node"] = json!(node);
			if !self.decide(&resource, "registry.read").await?
				|| !self.decide(&resource, "agent.execute").await?
			{
				return Ok(false);
			}
			nodes.entry(node).or_default().push(Reference {
				entry: EntityRef { id, version },
				digest: hash,
			});
		}
		for (node, references) in nodes {
			if self.unavailable_peers.contains(&node) {
				return Ok(false);
			}
			let resource = self.resource("node", &node, json!({"remote_node":node}));
			if !self.decide(&resource, "federation.discover").await? {
				return Ok(false);
			}
			let peer: Option<Peer> = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
					))
					.from(sea_orm::sea_query::Alias::new("peers"))
					.and_where(sea_orm::sea_query::Expr::cust("node_id = $1 AND enabled"))
					.lock(sea_orm::sea_query::LockType::Share)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&node)
			.fetch_optional(&mut **self.tx)
			.await?;
			let Some(peer) = peer else {
				return Ok(false);
			};
			if peer.protocol_version != crate::config::PROTOCOL_VERSION {
				return Ok(false);
			}
			let token = match crate::config::peer_secret(&peer.credential_env) {
				Ok(token) => token,
				Err(_) => return Ok(false),
			};
			for references in references.chunks(128) {
				let response = self.peer_client.post(format!("{}/federation/v0.1/scoped/registry/verify",peer.endpoint.trim_end_matches('/')))
                    .timeout(std::time::Duration::from_secs(10))
                    .bearer_auth(&token).header("x-aidash-node",&self.node_id).header("x-aidash-protocol",crate::config::PROTOCOL_VERSION)
                    .json(&json!({"tenant":self.identity.tenant,"subject":self.identity.subject,"references":references})).send().await;
				let response = match response {
					Ok(response) if response.status().is_success() => response,
					_ => {
						self.unavailable_peers.insert(node.clone());
						return Ok(false);
					}
				};
				if !crate::response::json::<bool>(response, 1024)
					.await
					.unwrap_or(false)
				{
					return Ok(false);
				}
			}
		}
		Ok(true)
	}
}
