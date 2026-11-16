//! Dependency reads retain their original query trees and borrowed transaction.
use crate::{
	authorization::{access::Access, catalog},
	registry::{EntityRef, Entry},
};
use aidash_application::{Result, ports::federation::dependencies::DependencyScope};
use aidash_domain::{Run, federation::dependencies::Reference, policy::Resource};
use async_trait::async_trait;
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct NativeDependencies<'a>(pub(crate) &'a mut Access);
impl aidash_application::ports::authorization::visibility::RowOutcome for NativeDependencies<'_> {
	fn row_silence(&self) -> Option<String> {
		self.0.row_silence()
	}
	fn take_row_silence(&mut self) -> Option<String> {
		self.0.take_row_silence()
	}
	fn set_row_silence(&mut self, node: Option<String>) {
		self.0.set_row_silence(node);
	}
}
#[async_trait]
impl DependencyScope for NativeDependencies<'_> {
	fn node(&self) -> &str {
		self.0.node_reference()
	}
	fn identity(&self) -> (&str, &str) {
		(&self.0.identity.tenant, &self.0.identity.subject)
	}
	fn start_frontier(&mut self) {
		self.0.dependency_frontier = Some(vec![]);
	}
	fn take_frontier(&mut self) -> Vec<Reference> {
		self.0.dependency_frontier.take().unwrap_or_default()
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.0.resource(kind, id, attributes)
	}
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		catalog::resource(self.0, entry)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.0.decide(resource, action).await.map_err(Into::into)
	}
	async fn admission(&mut self, admission_id: Uuid, home_node: &str) -> Result<Option<Run>> {
		let admission_id = &admission_id;
		let run: Option<crate::domain::Run> = {
			let query_bind_1 = admission_id;
			let query_bind_2 = home_node;
			aidash_server::database::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("runs"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND home_node=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.0.tx)
			.await?
		};
		Ok(run)
	}
	async fn bound_grant(&mut self, admission_id: Uuid) -> Result<Option<Uuid>> {
		let admission_id = &admission_id;
		let bound: Option<Uuid> = {
			let query_bind_1 = admission_id;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("grant_id"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **self.0.tx)
			.await?
		};
		Ok(bound)
	}
	async fn run_visible(&mut self, run: &Run) -> Result<bool> {
		Box::pin(self.0.run_visible(run)).await.map_err(Into::into)
	}
	async fn grant_visible(
		&mut self,
		execution_node: &str,
		grant_id: Uuid,
		admission_id: Uuid,
	) -> Result<bool> {
		Box::pin(crate::authorization::remote::reads::visible(
			self.0,
			execution_node,
			grant_id,
			admission_id,
		))
		.await
		.map_err(Into::into)
	}
	async fn readable_entry(&mut self, reference: &EntityRef) -> Result<Entry> {
		catalog::readable(self.0, reference)
			.await
			.map_err(Into::into)
	}
	async fn route(
		&mut self,
		node: &str,
	) -> Result<Option<aidash_domain::federation::route::Route>> {
		self.0.route_to(node).await.map_err(Into::into)
	}
	fn unavailable(&self, node: &str) -> bool {
		self.0.peer_unavailable(node)
	}
	fn mark_unavailable(&mut self, node: &str) {
		self.0.mark_peer_unavailable(node);
	}
	fn skipped(&mut self, node: &str) {
		self.0.skip_peer(node);
	}
	fn answered(&mut self, node: &str) {
		self.0.peer_answered(node);
	}
}
