//! Bounded dashboard candidate queries in the caller's authorization transaction.
use super::Reads;
use aidash_application::{
	Error, Result,
	ports::authorization::state::{WorkspaceListingScope, WorkspaceStateScope},
};
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, RunInspection, RunMetadata, Task, Workspace,
	policy::Resource,
	registry::{Entry, Search},
	run_state::RawRun,
	workspaces::TaskPage,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder as _, SelectStatement,
};
use uuid::Uuid;

#[async_trait]
impl WorkspaceListingScope for Reads<'_> {
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
		aidash_application::authorization::workspaces::visible(self, action).await
	}
	async fn task_page(&mut self, ids: &[Uuid], offset: u64) -> Result<TaskPage> {
		aidash_application::authorization::projection::task_page(self, ids, offset).await
	}
}

pub(crate) struct StateReads<'a> {
	pub access: &'a mut crate::apps::identity::services::access::Access,
	pub federation: &'a crate::federation::Federation,
}
impl StateReads<'_> {
	fn candidates(&self, table: &str, node: Option<&str>, order: &str) -> SelectStatement {
		let mut q = Query::select();
		q.column(Asterisk)
			.from(Alias::new(table))
			.and_where(
				Expr::col(Alias::new("tenant"))
					.eq(Expr::value(self.access.identity.tenant.clone())),
			)
			.order_by(Alias::new(order), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(50);
		if let Some(node) = node {
			if table == "workspaces" {
				q.and_where(Expr::col(Alias::new("node")).eq(Expr::value(node.to_owned())));
			} else {
				let mut ids = Query::select();
				ids.column(Alias::new("id"))
					.from(Alias::new("workspaces"))
					.and_where(
						Expr::col(Alias::new("tenant"))
							.eq(Expr::value(self.access.identity.tenant.clone())),
					)
					.and_where(Expr::col(Alias::new("node")).eq(Expr::value(node.to_owned())));
				q.and_where(Expr::col(Alias::new("workspace_id")).in_subquery(ids.to_owned()));
			}
		}
		q
	}
}
#[async_trait]
impl WorkspaceStateScope for StateReads<'_> {
	async fn allowed(&mut self, id: Uuid, action: &str) -> Result<bool> {
		self.access.allowed(id, action).await.map_err(Into::into)
	}
	async fn registry(&mut self) -> Result<Vec<Entry>> {
		aidash_application::authorization::catalog::list(
			&mut crate::bootstrap::catalog_scope(self.access),
			&Search::default(),
		)
		.await
	}
	async fn latest_visible_events(&mut self, node: Option<&str>) -> Result<Vec<Event>> {
		super::projection::state_events(
			&mut Reads {
				access: self.access,
			},
			node,
		)
		.await
	}
	async fn workspace_candidates(
		&mut self,
		node: Option<&str>,
		cursor: Option<Uuid>,
	) -> Result<Vec<Uuid>> {
		let mut q = self.candidates("workspaces", node, "created_at");
		if let Some(id) = cursor {
			let mut position = self.candidates("workspaces", node, "created_at");
			position.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)));
			let row: Option<Workspace> =
				aidash_server::database::query_as(&position.to_string(PostgresQueryBuilder))
					.fetch_optional(&mut **self.access.tx)
					.await
					.map_err(Error::from)?;
			let at = row
				.ok_or_else(|| Error::Invalid("invalid cursor".into()))?
				.created_at;
			q.cond_where(
				Condition::any()
					.add(Expr::col(Alias::new("created_at")).lt(Expr::value(at)))
					.add(
						Condition::all()
							.add(Expr::col(Alias::new("created_at")).eq(Expr::value(at)))
							.add(Expr::col(Alias::new("id")).lt(Expr::value(id))),
					),
			);
		}
		let rows: Vec<Workspace> =
			aidash_server::database::query_as(&q.to_string(PostgresQueryBuilder))
				.fetch_all(&mut **self.access.tx)
				.await
				.map_err(Error::from)?;
		Ok(rows.into_iter().map(|row| row.id).collect())
	}
	async fn workspace_rows(&mut self, ids: &[Uuid]) -> Result<Vec<Workspace>> {
		let q = Query::select()
			.column(Asterisk)
			.from(Alias::new("workspaces"))
			.and_where(Expr::col(Alias::new("id")).is_in(ids.iter().copied().map(Expr::value)))
			.order_by(Alias::new("created_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.to_string(PostgresQueryBuilder);
		aidash_server::database::query_as(&q)
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn task_candidates(&mut self, node: Option<&str>) -> Result<Vec<Task>> {
		let q = self
			.candidates("tasks", node, "created_at")
			.to_string(PostgresQueryBuilder);
		aidash_server::database::query_as(&q)
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn artifact_candidates(&mut self, node: Option<&str>) -> Result<Vec<Artifact>> {
		let q = self
			.candidates("artifacts", node, "created_at")
			.to_string(PostgresQueryBuilder);
		aidash_server::database::query_as(&q)
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn run_candidates(&mut self, node: Option<&str>) -> Result<Vec<RawRun>> {
		let q = self
			.candidates("runs", node, "updated_at")
			.to_string(PostgresQueryBuilder);
		aidash_server::database::query_as(&q)
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn pending_human_candidates(&mut self, node: Option<&str>) -> Result<Vec<HumanRequest>> {
		let q = self
			.candidates("human_requests", node, "created_at")
			.and_where(Expr::col(Alias::new("response")).is_null())
			.to_string(PostgresQueryBuilder);
		aidash_server::database::query_as(&q)
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn conversation_candidates(&mut self, node: Option<&str>) -> Result<Vec<Conversation>> {
		let q = self
			.candidates("conversations", node, "created_at")
			.to_string(PostgresQueryBuilder);
		aidash_server::database::query_as(&q)
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn task_visible(&mut self, row: &Task) -> Result<bool> {
		self.access.task_visible(row).await.map_err(Into::into)
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		self.access.artifact_visible(row).await.map_err(Into::into)
	}
	async fn run_visible(&mut self, row: &RunMetadata) -> Result<bool> {
		self.access.run_visible(row).await.map_err(Into::into)
	}
	async fn visible_run(&mut self, id: Uuid) -> Result<Option<RunInspection>> {
		match crate::apps::identity::repositories::execution::details::visible_within(
			self.federation,
			self.access,
			id,
		)
		.await
		{
			Ok(run) => Ok(Some(run)),
			Err(Error::Forbidden) => Ok(None),
			Err(error) => Err(error),
		}
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
}
