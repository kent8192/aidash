//! Journal selection and enabled peer locks borrow the original reader transaction.
use super::{NativeReads, Reads};
use crate::{Result as NativeResult, apps::identity::services::access::Access};
use aidash_application::{
	Result,
	ports::federation::registry_reads::{
		RegistryJournalRow, RegistryReadScope, RegistryVerificationScope,
	},
};
use aidash_domain::{
	federation::{Peer, dependencies::Reference as Dependency},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::query::{Expr, QueryStatementBuilder as _, SimpleExpr};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl RegistryReadScope for Reads<'_> {
	fn protocol(&self) -> &str {
		crate::config::PROTOCOL_VERSION
	}
	fn identity(&self) -> (&str, &str) {
		(&self.access.identity.tenant, &self.access.identity.subject)
	}
	fn cached(&self, run: Uuid) -> Option<bool> {
		self.access
			.dependency_frontier
			.is_none()
			.then(|| self.access.cached_remote_read(run))
			.flatten()
	}
	fn remember(&mut self, run: Uuid, visible: bool) {
		if self.access.dependency_frontier.is_none() {
			self.access.remember_remote_read(run, visible);
		}
	}
	fn frontier(&mut self) -> Option<&mut Vec<Dependency>> {
		self.access.dependency_frontier.as_mut()
	}
	fn unavailable(&self, node: &str) -> bool {
		self.access.peer_unavailable(node)
	}
	fn mark_unavailable(&mut self, node: &str) {
		self.access.mark_peer_unavailable(node);
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		crate::authorization::catalog::resource(self.access, entry)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<RegistryJournalRow>> {
		let result: NativeResult<Vec<RegistryJournalRow>> = async {
			let rows: Vec<(String, String, String, String, Value)> = {
				let query_bind_1 = run;
				crate::database::native::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("node_id")),
						))
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("entry_id")),
						))
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new(
								"entry_version",
							)),
						))
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("digest")),
						))
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("metadata")),
						))
						.from(reinhardt::query::Alias::new(
							"authorization_run_remote_reads",
						))
						.and_where(SimpleExpr::CustomWithExpr(
							"(run_id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("node_id"),
							)),
							reinhardt::query::Order::Asc,
						)
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("entry_id"),
							)),
							reinhardt::query::Order::Asc,
						)
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("entry_version"),
							)),
							reinhardt::query::Order::Asc,
						)
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("digest"),
							)),
							reinhardt::query::Order::Asc,
						)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.columns(&["node_id", "entry_id", "entry_version", "digest", "metadata"])
				.fetch_all(&mut **self.access.tx)
				.await?
			};
			Ok(rows)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		let result: NativeResult<Option<Peer>> = async {
			let peer: Option<Peer> = {
				let query_bind_1 = node;
				crate::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new("peers"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(node_id = ? AND enabled)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Share)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			};
			Ok(peer)
		}
		.await;
		result.map_err(Into::into)
	}
}

#[async_trait]
impl RegistryReadScope for NativeReads<'_> {
	fn protocol(&self) -> &str {
		crate::config::PROTOCOL_VERSION
	}
	fn identity(&self) -> (&str, &str) {
		self.access.remote_identity()
	}
	fn cached(&self, run: Uuid) -> Option<bool> {
		self.access.cached_remote_read(run)
	}
	fn remember(&mut self, run: Uuid, visible: bool) {
		self.access.remember_remote_read(run, visible);
	}
	fn frontier(&mut self) -> Option<&mut Vec<Dependency>> {
		None
	}
	fn unavailable(&self, node: &str) -> bool {
		self.access.peer_unavailable(node)
	}
	fn mark_unavailable(&mut self, node: &str) {
		self.access.mark_peer_unavailable(node);
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		self.access.resource(
			&entry.kind,
			&entry.id,
			aidash_application::authorization::catalog::attributes(entry),
		)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<RegistryJournalRow>> {
		crate::apps::identity::models::AuthorizationRunRemoteRead::for_run(
			self.access.tx.as_mut(),
			run,
		)
		.await
		.map_err(Into::into)
	}
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		crate::apps::federation::peer::models::Peer::enabled_in(self.access.tx.as_mut(), node)
			.await
			.map_err(Into::into)
	}
}
pub(crate) struct VerificationScope<'a> {
	pub access: &'a mut Access,
	pub node: &'a str,
}
#[async_trait]
impl RegistryVerificationScope for VerificationScope<'_> {
	fn node(&self) -> &str {
		self.node
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		crate::authorization::catalog::resource(self.access, entry)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn entry(&mut self, reference: &EntityRef) -> Result<Entry> {
		crate::authorization::catalog::entry(self.access, reference, "registry.read")
			.await
			.map_err(Into::into)
	}
}
