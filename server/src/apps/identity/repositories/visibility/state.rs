//! State projection reads preserve query ordering, candidate limits, raw Run rows and the caller's transaction.
use super::Reads;
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	ports::authorization::state::{WorkspaceListingScope, WorkspaceStateScope},
};
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, RunMetadata, Workspace,
	policy::Resource,
	registry::{Entry, Search},
	run_state::RawRun,
	workspaces::TaskPage,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use uuid::Uuid;
#[async_trait]
impl WorkspaceListingScope for Reads<'_> {
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
		aidash_application::authorization::workspaces::visible(self, action).await
	}
	async fn task_page(&mut self, workspaces: &[Uuid], offset: u64) -> Result<TaskPage> {
		aidash_application::authorization::projection::task_page(self, workspaces, offset).await
	}
}
#[async_trait]
impl WorkspaceStateScope for Reads<'_> {
	async fn allowed(&mut self, id: Uuid, action: &str) -> Result<bool> {
		aidash_application::authorization::workspaces::allowed(self, id, action).await
	}
	async fn registry(&mut self) -> Result<Vec<Entry>> {
		aidash_application::authorization::catalog::list(
			&mut crate::bootstrap::catalog_scope(self.access),
			&Search::default(),
		)
		.await
	}
	async fn latest_visible_events(
		&mut self,
		workspaces: &[Uuid],
		include_marketplace: bool,
	) -> Result<Vec<Event>> {
		aidash_application::authorization::projection::latest_visible_events(
			self,
			workspaces,
			include_marketplace,
		)
		.await
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		self.access.artifact_visible(row).await.map_err(Into::into)
	}
	async fn run_visible(&mut self, row: &RunMetadata) -> Result<bool> {
		self.access.run_visible(row).await.map_err(Into::into)
	}
	async fn human_resource(&mut self, row: &HumanRequest) -> Result<Resource> {
		self.access.human_resource(row).await.map_err(Into::into)
	}
	async fn conversation_resource(&mut self, row: &Conversation) -> Result<Resource> {
		self.access
			.conversation_resource(row)
			.await
			.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn workspace_rows(&mut self, visible: &[Uuid]) -> Result<Vec<Workspace>> {
		let result: NativeResult<Vec<Workspace>> = async {
			Ok({
				let query_bind_1 = visible;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("workspaces"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=ANY(?))".to_owned(),
							vec![crate::database::uuid_array(query_bind_1.to_owned())],
						))
						.order_by(Alias::new("created_at"), Order::Desc)
						.order_by(Alias::new("id"), Order::Asc)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn artifact_rows(&mut self, visible: &[Uuid], offset: u64) -> Result<Vec<Artifact>> {
		let result: NativeResult<Vec<Artifact>> = async {
			Ok({
				let query_bind_1 = visible;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("artifacts"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id=ANY(?))".to_owned(),
							vec![crate::database::uuid_array(query_bind_1.to_owned())],
						))
						.order_by(Alias::new("created_at"), Order::Desc)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(500)
						.offset(offset)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn raw_runs(&mut self, visible: &[Uuid], offset: i64) -> Result<Vec<RawRun>> {
		let result: NativeResult<Vec<RawRun>> = async {
			Ok({
				let query_bind_1 = visible;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id=ANY(?))".to_owned(),
							vec![crate::database::uuid_array(query_bind_1.to_owned())],
						))
						.order_by(Alias::new("updated_at"), Order::Desc)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(500)
						.offset(offset as u64)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn human_rows(&mut self, run_ids: &[Uuid], offset: i64) -> Result<Vec<HumanRequest>> {
		let result: NativeResult<Vec<HumanRequest>> = async {
			Ok({
				let query_bind_1 = run_ids;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("human_requests"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(run_id=ANY(?))".to_owned(),
							vec![crate::database::uuid_array(query_bind_1.to_owned())],
						))
						.order_by(Alias::new("created_at"), Order::Desc)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(500)
						.offset(offset as u64)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn conversation_rows(
		&mut self,
		visible: &[Uuid],
		offset: i64,
	) -> Result<Vec<Conversation>> {
		let result: NativeResult<Vec<Conversation>> = async {
			Ok({
				let query_bind_1 = visible;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("conversations"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id=ANY(?))".to_owned(),
							vec![crate::database::uuid_array(query_bind_1.to_owned())],
						))
						.order_by(Alias::new("created_at"), Order::Desc)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(500)
						.offset(offset as u64)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
}
