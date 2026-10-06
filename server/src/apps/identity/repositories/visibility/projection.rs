//! Queries preserve ordering, bounds, typed decoding and the caller's transaction.
use super::Reads;
use crate::Result as NativeResult;
use aidash_application::{Result, ports::authorization::projection::WorkspaceProjection};
use aidash_domain::{
	Artifact, Event, Message, Task, Workspace, WorkspaceSnapshot, policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, BinOper, ColumnRef, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr, types::PgBinOper,
};
use serde_json::json;
use uuid::Uuid;
pub(crate) fn event_scope(
	include_marketplace: bool,
	tenant: &str,
	workspaces: &[Uuid],
) -> Condition {
	let scope = Condition::any().add(
		Expr::col(Alias::new("workspace_id")).is_in(workspaces.iter().copied().map(Expr::value)),
	);
	if include_marketplace {
		scope.add(
			Condition::all()
				.add(Expr::col(Alias::new("workspace_id")).is_null())
				.add(Expr::col(Alias::new("kind")).like("marketplace.%"))
				.add(
					Expr::col(Alias::new("kind"))
						.ne(reinhardt::query::Expr::value("marketplace.audit")),
				)
				.add(
					Condition::any()
						.add(
							Expr::cust("data->>'tenant'").eq(reinhardt::query::Expr::value(tenant)),
						)
						.add(
							Expr::cust("data->>'key'").in_subquery(
								Query::select()
									.column(Alias::new("key"))
									.from_as(
										Alias::new("marketplace_audiences"),
										Alias::new("audience"),
									)
									.and_where(Expr::cust("audience.document->'tenants'").binary(
										BinOper::PgOperator(PgBinOper::Contains),
										Expr::val(json!([tenant])),
									))
									.to_owned(),
							),
						),
				),
		)
	} else {
		scope
	}
}
#[async_trait]
impl WorkspaceProjection for Reads<'_> {
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
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
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
	async fn track(&mut self, snapshot: &WorkspaceSnapshot) -> Result<()> {
		self.access
			.track_snapshot(snapshot)
			.await
			.map_err(Into::into)
	}
	async fn task_rows(&mut self, workspaces: &[Uuid], cursor: u64) -> Result<Vec<Task>> {
		let result: NativeResult<Vec<Task>> = async {
			let rows: Vec<Task> = aidash_server::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("tasks"))
					.and_where(
						Expr::col(Alias::new("workspace_id"))
							.is_in(workspaces.iter().copied().map(Expr::value)),
					)
					.order_by(Alias::new("created_at"), Order::Desc)
					.order_by(Alias::new("id"), Order::Desc)
					.limit(500)
					.offset(cursor)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.access.tx)
			.await?;
			Ok(rows)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event_rows(
		&mut self,
		workspaces: &[Uuid],
		include_marketplace: bool,
		cursor: i64,
		page_size: usize,
	) -> Result<Vec<Event>> {
		let result: NativeResult<Vec<Event>> = async {
			let rows: Vec<Event> = aidash_server::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("events"))
					.and_where(event_scope(
						include_marketplace,
						&self.access.identity.tenant,
						workspaces,
					))
					.and_where(Expr::col(Alias::new("sequence")).lt(Expr::value(cursor)))
					.order_by(Alias::new("sequence"), Order::Desc)
					.limit(page_size as u64)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.access.tx)
			.await?;
			Ok(rows)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn message_rows(&mut self, workspace: Uuid, offset: u64) -> Result<Vec<Message>> {
		let result: NativeResult<Vec<Message>> = async {
			let rows: Vec<Message> = aidash_server::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("messages"))
					.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(workspace)))
					.order_by(Alias::new("created_at"), Order::Desc)
					.order_by(Alias::new("id"), Order::Desc)
					.limit(100)
					.offset(offset)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.access.tx)
			.await?;
			Ok(rows)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn workspace_row(&mut self, id: Uuid) -> Result<Workspace> {
		let result: NativeResult<Workspace> = async {
			Ok({
				let query_bind_1 = id;
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
	async fn workspace_tasks(&mut self, id: Uuid) -> Result<Vec<Task>> {
		let result: NativeResult<Vec<Task>> = async {
			Ok({
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
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
						.order_by(Alias::new("created_at"), Order::Asc)
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
	async fn workspace_artifacts(&mut self, id: Uuid) -> Result<Vec<Artifact>> {
		let result: NativeResult<Vec<Artifact>> = async {
			Ok({
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("artifacts"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"workspace_id",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
						)
						.order_by(Alias::new("created_at"), Order::Asc)
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
}
