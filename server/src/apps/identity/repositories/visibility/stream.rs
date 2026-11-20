//! Unlocked idle snapshots and locked stream pages preserve the original SQL.
use super::{Reads, projection::event_scope};
use crate::{Result as NativeResult, apps::identity::services::thread::Threads};
use aidash_application::{
	Result,
	ports::authorization::{
		stream::{AuthorityRecord, StreamAuthorityStore, StreamSession},
		visibility::RunVerdicts,
	},
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
	pub(crate) threads: &'a Threads,
}
#[async_trait]
impl StreamAuthorityStore for Authority<'_> {
	fn tenant(&self) -> &str {
		&self.threads.identity.tenant
	}
	fn subject(&self) -> &str {
		&self.threads.identity.subject
	}
	fn node_id(&self) -> &str {
		self.threads.store.deployment.as_str()
	}
	fn reports_status(&self, issuer: &str) -> bool {
		if self
			.threads
			.store
			.pool
			.dashboard_policy()
			.is_some_and(|policy| policy.authority.is_some())
		{
			return false;
		}
		crate::authorization::identity::reports_status(issuer)
	}
	fn now(&self) -> chrono::DateTime<chrono::Utc> {
		chrono::Utc::now()
	}
	async fn current(&self, thread: Option<Uuid>) -> Result<Option<AuthorityRecord>> {
		let result: NativeResult<Option<AuthorityRecord>> = async {
			self.threads
				.identity
				.check_binding(&self.threads.store.pool)
				.await?;
			struct Current {
				revision: i64,
				document: Value,
				owner_subject: Option<String>,
				duty_id: Option<Uuid>,
				node: Option<String>,
				mapping_id: Option<Uuid>,
				mapping_enabled: Option<bool>,
				mapping_tenant: Option<String>,
				issuer: Option<String>,
				last_valid_at: Option<chrono::DateTime<chrono::Utc>>,
				disabled_at: Option<chrono::DateTime<chrono::Utc>>,
			}
			crate::native_record!(Current {
				revision,
				document,
				owner_subject,
				duty_id,
				node,
				mapping_id,
				mapping_enabled,
				mapping_tenant,
				issuer,
				last_valid_at,
				disabled_at
			});

			let col =
				|table: &str, column: &str| Expr::col((Alias::new(table), Alias::new(column)));
			let mut query = Query::select();
			query
				.columns([
					(Alias::new("b"), Alias::new("revision")),
					(Alias::new("b"), Alias::new("document")),
					(Alias::new("w"), Alias::new("owner_subject")),
				])
				// The selected Thread's Duty, from the same statement (#1008).
				.expr_as(
					crate::apps::workspaces::repositories::duties::own_duty(
						col("w", "thread_id").into(),
						col("w", "tenant").into(),
					),
					Alias::new("duty_id"),
				)
				// The selected Thread's Node, for its administrator (#1008).
				.expr_as(col("t", "node"), Alias::new("node"))
				.expr_as(col("m", "id"), Alias::new("mapping_id"))
				.expr_as(col("m", "enabled"), Alias::new("mapping_enabled"))
				.expr_as(col("m", "tenant"), Alias::new("mapping_tenant"))
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
						Alias::new("authorization_threads"),
						Alias::new("w"),
					),
					Condition::all()
						.add(col("w", "thread_id").eq(Expr::value(thread)))
						.add(col("w", "tenant").equals((Alias::new("c"), Alias::new("tenant")))),
				)
				.join(
					reinhardt::query::JoinType::LeftJoin,
					reinhardt::query::TableRef::table_alias(
						Alias::new("threads"),
						Alias::new("t"),
					),
					Condition::all()
						.add(col("t", "id").equals((Alias::new("w"), Alias::new("thread_id"))))
						.add(col("t", "tenant").equals((Alias::new("w"), Alias::new("tenant")))),
				)
				.and_where(col("c", "id").eq(Expr::cust("$1")))
				.and_where(col("c", "tenant").eq(Expr::cust("$2")))
				.and_where(col("c", "subject").eq(Expr::cust("$3")))
				.and_where(col("c", "revoked_at").is_null())
				.and_where(col("c", "expires_at").gt(Expr::cust("clock_timestamp()")));
			if let Some(thread) = thread {
				query.and_where(
					crate::apps::identity::repositories::nodes::live_thread_condition(
						Expr::value(thread).into(),
						Expr::value(self.threads.identity.tenant.clone()).into(),
					),
				);
			}
			let current: Option<Current> =
				crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
					.bind(self.threads.identity.credential_id)
					.bind(&self.threads.identity.tenant)
					.bind(&self.threads.identity.subject)
					.fetch_optional(
						self.threads
							.store
							.pool
							.in_scope(&self.threads.identity.tenancy()?),
					)
					.await?;
			if let Some(current) = &current {
				let snapshot = crate::authorization::Snapshot {
					revision: current.revision,
					bundle: serde_json::from_value(current.document.clone())?,
				};
				self.threads
					.store
					.pool
					.atomic(&self.threads.identity.tenancy()?, async |tx| {
						crate::authorization::identity::require_live_subject(
							tx,
							&snapshot,
							&self.threads.identity.subject,
						)
						.await
					})
					.await?;
			}
			if let Some(current) = &current
				&& current.mapping_id.is_some()
			{
				let tenant = current
					.mapping_tenant
					.as_deref()
					.ok_or(crate::Error::Forbidden)?;
				if tenant != self.threads.identity.tenant {
					return Err(crate::Error::Forbidden);
				}
				crate::authorization::identity::require_dashboard_mapping(
					self.threads.store.pool.dashboard_policy().as_ref(),
					current.issuer.as_deref().ok_or(crate::Error::Forbidden)?,
					tenant,
				)?;
			}
			Ok(current.map(|current| AuthorityRecord {
				revision: current.revision,
				document: current.document,
				owner_subject: current.owner_subject,
				duty_id: current.duty_id,
				node: current.node,
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
	async fn event_threads(&mut self, thread: Option<Uuid>) -> Result<Vec<Uuid>> {
		Box::pin(self.access.event_threads(thread))
			.await
			.map_err(Into::into)
	}
	async fn require_thread(&mut self, thread: Uuid, action: &str) -> Result<()> {
		Box::pin(self.access.require_thread(thread, action))
			.await
			.map_err(Into::into)
	}
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
		Box::pin(self.access.visible(action))
			.await
			.map_err(Into::into)
	}
	async fn allowed(&mut self, thread: Uuid, action: &str) -> Result<bool> {
		Box::pin(self.access.allowed(thread, action))
			.await
			.map_err(Into::into)
	}
	async fn event_visible(&mut self, event: &Event, runs: &mut RunVerdicts) -> Result<bool> {
		Box::pin(self.access.event_visible_in(event, runs))
			.await
			.map_err(Into::into)
	}
	async fn stream_rows(
		&mut self,
		after: i64,
		thread: Option<Uuid>,
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
								thread.is_none(),
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
		thread: Option<Uuid>,
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
								thread.is_none(),
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
