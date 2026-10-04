//! Unlocked idle snapshots and locked stream pages preserve the original SQL.
use super::{Reads, projection::event_scope};
use crate::{Result as NativeResult, apps::identity::services::workspace::Workspaces};
use aidash_application::{
	Result,
	ports::authorization::stream::{AuthorityRecord, StreamAuthorityStore, StreamSession},
};
use aidash_domain::Event;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Authority<'a> {
	pub(crate) workspaces: &'a Workspaces,
}
#[async_trait]
impl StreamAuthorityStore for Authority<'_> {
	fn tenant(&self) -> &str {
		&self.workspaces.identity.tenant
	}
	fn subject(&self) -> &str {
		&self.workspaces.identity.subject
	}
	fn node_id(&self) -> &str {
		&self.workspaces.store.node_id
	}
	fn google_issuer(&self) -> &str {
		crate::config::GOOGLE_OIDC_ISSUER
	}
	fn now(&self) -> chrono::DateTime<chrono::Utc> {
		chrono::Utc::now()
	}
	async fn current(&self, workspace: Option<Uuid>) -> Result<Option<AuthorityRecord>> {
		let result: NativeResult<Option<AuthorityRecord>> = async {
			#[derive(sqlx::FromRow)]
			struct Current {
				revision: i64,
				document: Value,
				owner_subject: Option<String>,
				mapping_id: Option<Uuid>,
				mapping_enabled: Option<bool>,
				issuer: Option<String>,
				last_valid_at: Option<chrono::DateTime<chrono::Utc>>,
				disabled_at: Option<chrono::DateTime<chrono::Utc>>,
			}
			let col =
				|table: &str, column: &str| Expr::col((Alias::new(table), Alias::new(column)));
			let mut query = Query::select();
			query
				.columns([
					(Alias::new("b"), Alias::new("revision")),
					(Alias::new("b"), Alias::new("document")),
					(Alias::new("w"), Alias::new("owner_subject")),
				])
				.expr_as(col("m", "id"), Alias::new("mapping_id"))
				.expr_as(col("m", "enabled"), Alias::new("mapping_enabled"))
				.columns(
					["issuer", "last_valid_at", "disabled_at"]
						.map(|name| (Alias::new("i"), Alias::new(name))),
				)
				.from_as(Alias::new("authorization_credentials"), Alias::new("c"))
				.join(
					reinhardt::query::JoinType::InnerJoin,
					reinhardt::query::TableRef::table_alias(
						Alias::new("authorization_bundles"),
						Alias::new("b"),
					),
					col("b", "tenant").equals((Alias::new("c"), Alias::new("tenant"))),
				)
				.join(
					reinhardt::query::JoinType::LeftJoin,
					reinhardt::query::TableRef::table_alias(
						Alias::new("dashboard_mappings"),
						Alias::new("m"),
					),
					col("m", "credential_id").equals((Alias::new("c"), Alias::new("id"))),
				)
				.join(
					reinhardt::query::JoinType::LeftJoin,
					reinhardt::query::TableRef::table_alias(
						Alias::new("dashboard_identities"),
						Alias::new("i"),
					),
					col("i", "id").equals((Alias::new("m"), Alias::new("identity_id"))),
				)
				.join(
					reinhardt::query::JoinType::LeftJoin,
					reinhardt::query::TableRef::table_alias(
						Alias::new("authorization_workspaces"),
						Alias::new("w"),
					),
					Condition::all()
						.add(col("w", "workspace_id").eq(Expr::cust("$4")))
						.add(col("w", "tenant").equals((Alias::new("c"), Alias::new("tenant")))),
				)
				.and_where(col("c", "id").eq(Expr::cust("$1")))
				.and_where(col("c", "tenant").eq(Expr::cust("$2")))
				.and_where(col("c", "subject").eq(Expr::cust("$3")))
				.and_where(col("c", "revoked_at").is_null())
				.and_where(col("c", "expires_at").gt(Expr::cust("clock_timestamp()")));
			let current: Option<Current> = sqlx::query_as(&query.to_string(PostgresQueryBuilder))
				.bind(self.workspaces.identity.credential_id)
				.bind(&self.workspaces.identity.tenant)
				.bind(&self.workspaces.identity.subject)
				.bind(workspace)
				.fetch_optional(&self.workspaces.store.pool)
				.await?;
			Ok(current.map(|current| AuthorityRecord {
				revision: current.revision,
				document: current.document,
				owner_subject: current.owner_subject,
				mapping_id: current.mapping_id,
				mapping_enabled: current.mapping_enabled,
				issuer: current.issuer,
				last_valid_at: current.last_valid_at,
				disabled_at: current.disabled_at,
			}))
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl StreamSession for Reads<'_> {
	async fn event_workspaces(&mut self, workspace: Option<Uuid>) -> Result<Vec<Uuid>> {
		Box::pin(self.access.event_workspaces(workspace))
			.await
			.map_err(Into::into)
	}
	async fn require_workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		Box::pin(self.access.require_workspace(workspace, action))
			.await
			.map_err(Into::into)
	}
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
		Box::pin(self.access.visible(action))
			.await
			.map_err(Into::into)
	}
	async fn allowed(&mut self, workspace: Uuid, action: &str) -> Result<bool> {
		Box::pin(self.access.allowed(workspace, action))
			.await
			.map_err(Into::into)
	}
	async fn event_visible(&mut self, event: &Event) -> Result<bool> {
		Box::pin(self.access.event_visible(event))
			.await
			.map_err(Into::into)
	}
	async fn stream_rows(
		&mut self,
		after: i64,
		workspace: Option<Uuid>,
		visible: &[Uuid],
	) -> Result<Vec<Event>> {
		let result: NativeResult<Vec<Event>> = async {
			let batch: Vec<Event> = aidash_server::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("events"))
					.and_where(
						Condition::all()
							.add(event_scope(
								workspace.is_none(),
								&self.access.identity.tenant,
								visible,
							))
							.add(Expr::col(Alias::new("sequence")).gt(Expr::value(after.max(0)))),
					)
					.order_by(Alias::new("sequence"), Order::Asc)
					.limit(100)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.access.tx)
			.await?;
			Ok(batch)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn read_rows(
		&mut self,
		cursor: i64,
		workspace: Option<Uuid>,
		visible: &[Uuid],
		page_size: usize,
	) -> Result<Vec<Event>> {
		let result: NativeResult<Vec<Event>> = async {
			let batch: Vec<Event> = aidash_server::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("events"))
					.and_where(
						Condition::all()
							.add(event_scope(
								workspace.is_none(),
								&self.access.identity.tenant,
								visible,
							))
							.add(Expr::col(Alias::new("sequence")).gt(Expr::value(cursor))),
					)
					.order_by(Alias::new("sequence"), Order::Asc)
					.limit(page_size as u64)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.access.tx)
			.await?;
			Ok(batch)
		}
		.await;
		result.map_err(Into::into)
	}
}
