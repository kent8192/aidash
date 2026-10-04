//! Dependency reads retain their original query trees and borrowed transaction.
use crate::{
	authorization::{access::Access, catalog},
	registry::{EntityRef, Entry},
};
use aidash_application::{Result, ports::federation::dependencies::DependencyScope};
use aidash_domain::{
	Run,
	federation::{Peer, dependencies::Reference},
	policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, Expr, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct NativeDependencies<'a>(pub(crate) &'a mut Access);
#[async_trait]
impl DependencyScope for NativeDependencies<'_> {
	fn node(&self) -> &str {
		&self.0.node_id
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
			.await
			.map_err(crate::Error::from)?
		};
		Ok(run)
	}
	async fn bound_grant(&mut self, admission_id: Uuid) -> Result<Option<Uuid>> {
		let admission_id = &admission_id;
		let bound: Option<Uuid> = {
			let query_bind_1 = admission_id;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("grant_id"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.0.tx)
			.await
			.map_err(crate::Error::from)?
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
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.0, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		let peer: Option<Peer> = {
			let query_bind_1 = node;
			crate::database::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("peers"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(node_id=? AND enabled)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.0.tx)
			.await
			.map_err(crate::Error::from)?
		};
		Ok(peer)
	}
}
