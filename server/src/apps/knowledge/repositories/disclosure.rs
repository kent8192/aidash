//! Borrowed source readers keep exact Query trees, row locks and operator visibility adapters.
use super::access::Lease;
use crate::{Error, Result as NativeResult};
use aidash_application::{Result, ports::semantic::visibility::SemanticDisclosureScope};
use aidash_domain::{Artifact, Message, policy::Resource};
use async_trait::async_trait;
use reinhardt::query::{Expr, QueryStatementBuilder as _, SimpleExpr};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Disclosure<'a, 'scope> {
	pub lease: &'a mut Lease<'scope>,
}
#[async_trait]
impl SemanticDisclosureScope for Disclosure<'_, '_> {
	fn scoped(&self) -> bool {
		!matches!(self.lease, Lease::Operator(_))
	}
	async fn operator_visible(&mut self, workspace: Uuid) -> Result<bool> {
		crate::authorization::remote::operator::visible(self.lease.tx(), workspace)
			.await
			.map_err(Into::into)
	}
	async fn workspace_resource(&mut self, workspace: Uuid) -> Result<Resource> {
		self.lease
			.access()
			.ok_or(Error::Forbidden)?
			.workspace(workspace)
			.await
			.map_err(Into::into)
	}
	fn resource(&mut self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.lease
			.access()
			.expect("scoped semantic authority")
			.resource(kind, id, attributes)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.lease
			.access()
			.ok_or(Error::Forbidden)?
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn managed_memory(&mut self, id: Uuid) -> Result<Option<(String, String)>> {
		let result: NativeResult<Option<(String, String)>> = async {
			let entry_id = id;
			let managed: Option<(String, String)> = {
				let query_bind_1 = entry_id;
				crate::database::native::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("agent_id")),
						))
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new(
								"agent_version",
							)),
						))
						.from(reinhardt::query::Alias::new("semantic_agent_memory"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(entry_id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.columns(&["agent_id", "agent_version"])
				.fetch_optional(&mut **self.lease.access().expect("scoped semantic authority").tx)
				.await?
			};
			Ok(managed)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		let result: NativeResult<Option<Artifact>> = async {
			let row: Option<Artifact> = {
				let query_bind_1 = &id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new("artifacts"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ? AND workspace_id = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(reinhardt::query::LockType::Share)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.lease.tx())
				.await?
			};
			Ok(row)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>> {
		let result: NativeResult<Option<Message>> = async {
			let row: Option<Message> = {
				let query_bind_1 = &id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new("messages"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ? AND workspace_id = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(reinhardt::query::LockType::Share)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.lease.tx())
				.await?
			};
			Ok(row)
		}
		.await;
		result.map_err(Into::into)
	}

	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		Box::pin(
			self.lease
				.access()
				.ok_or(Error::Forbidden)?
				.artifact_visible(row),
		)
		.await
		.map_err(Into::into)
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		Box::pin(
			self.lease
				.access()
				.ok_or(Error::Forbidden)?
				.message_visible(row),
		)
		.await
		.map_err(Into::into)
	}
}
