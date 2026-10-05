//! Record reads keep workspace predicates and strict status decoding in one transaction.
use super::Reads;
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	ports::authorization::records::{ChildSummaryScope, ChildTaskRecord, WorkspaceRecordScope},
};
use aidash_domain::{
	Artifact, Event, Message, Task, Workspace, WorkspaceSnapshot, policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
#[async_trait]
impl WorkspaceRecordScope for Reads<'_> {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		Box::pin(self.access.workspace(id))
			.await
			.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		Box::pin(self.access.require(resource, action))
			.await
			.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		Box::pin(self.access.decide(resource, action))
			.await
			.map_err(Into::into)
	}
	async fn task_visible(&mut self, row: &Task) -> Result<bool> {
		Box::pin(self.access.task_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		Box::pin(self.access.artifact_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		Box::pin(self.access.message_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn event_visible(&mut self, row: &Event) -> Result<bool> {
		Box::pin(self.access.event_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn track(&mut self, snapshot: &WorkspaceSnapshot) -> Result<()> {
		aidash_application::authorization::journals::track_snapshot(self, snapshot).await
	}
	async fn workspace_row(&mut self, workspace_id: Uuid) -> Result<Workspace> {
		let result: NativeResult<Workspace> = async {
			Ok({
				let query_bind_1 = workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("workspaces"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn task_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("tasks"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn artifact_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Artifact>> {
		let result: NativeResult<Option<Artifact>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("artifacts"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn message_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Message>> {
		let result: NativeResult<Option<Message>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("messages"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event_record(&mut self, workspace_id: Uuid, id: Uuid) -> Result<Option<Event>> {
		let result: NativeResult<Option<Event>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("events"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
}
struct ChildTaskSummaryRow {
	id: Uuid,
	created_by: String,
	status: crate::domain::TaskStatus,
}
crate::native_record!(ChildTaskSummaryRow {
	id,
	created_by,
	status
});

#[async_trait]
impl ChildSummaryScope for Reads<'_> {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn visible(
		&mut self,
		workspace: &Resource,
		workspace_id: Uuid,
		id: Uuid,
		created_by: &str,
	) -> Result<bool> {
		self.access
			.task_summary_visible(workspace, workspace_id, id, created_by)
			.await
			.map_err(Into::into)
	}
	async fn track_tasks(&mut self, workspace: Uuid, tasks: &[Uuid]) -> Result<()> {
		aidash_application::authorization::journals::track_tasks(self, workspace, tasks).await
	}
	async fn children(
		&mut self,
		workspace_id: Uuid,
		parent_id: Uuid,
		after: Option<Uuid>,
	) -> Result<Vec<ChildTaskRecord>> {
		let result: NativeResult<Vec<ChildTaskRecord>> = async {
			let rows: Vec<ChildTaskSummaryRow> = {
				let query_bind_1 = workspace_id;
				let query_bind_2 = parent_id;
				let query_bind_3 = after;
				crate::database::native::query_as(
					&Query::select()
						.columns([
							Alias::new("id"),
							Alias::new("created_by"),
							Alias::new("status"),
						])
						.from(Alias::new("tasks"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"workspace_id",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("parent_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.and_where(
							Condition::any()
								.add(
									SimpleExpr::CustomWithExpr(
										"(?::uuid)".to_owned(),
										vec![Expr::value(query_bind_3.to_owned()).into()],
									)
									.is_null(),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.gt(SimpleExpr::CustomWithExpr(
											"(?::uuid)".to_owned(),
											vec![Expr::value(query_bind_3.to_owned()).into()],
										)),
								),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(100)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["id", "created_by", "status"])
				.fetch_all(&mut **self.access.tx)
				.await?
			};
			Ok(rows
				.into_iter()
				.map(|row| ChildTaskRecord {
					id: row.id,
					created_by: row.created_by,
					status: row.status,
				})
				.collect())
		}
		.await;
		result.map_err(Into::into)
	}
}
