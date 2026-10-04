//! Native lifecycle queries preserve caller-owned lock and mutation order.
use crate::{Error, Result, federation::Federation};
use aidash_application::{
	authorization::Snapshot, ports::generation::lifecycle::GenerationLifecycleScope,
};
use aidash_domain::generation::requests::{Control, Request};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
pub(crate) struct NativeLifecycle<'a, 'tx> {
	pub runtime: &'a Federation,
	pub transaction: &'a mut Transaction<'tx, Postgres>,
}
#[async_trait]
impl GenerationLifecycleScope for NativeLifecycle<'_, '_> {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}

	async fn unused(&mut self, job: &Request) -> aidash_application::Result<(i64, i64, i64)> {
		let tx = &mut *self.transaction;
		Ok({
			let query_bind_1 = job.id;
			sqlx::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust("token_limit - used_tokens"))
					.expr(reinhardt::query::Expr::cust(
						"compaction_call_limit - compaction_calls",
					))
					.expr(reinhardt::query::Expr::cust(
						"embedding_call_limit - embedding_calls",
					))
					.from(reinhardt::query::Alias::new("generation_budgets"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(request_id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await
			.map_err(Error::from)?
		})
	}

	async fn release_policy(
		&mut self,
		job: &Request,
		unused: i64,
		unused_calls: i64,
		unused_embeddings: i64,
	) -> aidash_application::Result<()> {
		let tx = &mut *self.transaction;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = &job.policy_id;
		let query_bind_3 = unused;
		let query_bind_4 = unused_calls;
		let query_bind_5 = unused_embeddings;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("generation_policies"))
				.value_expr(
					reinhardt::query::Alias::new("allocated_tokens"),
					SimpleExpr::CustomWithExpr(
						"(allocated_tokens - ?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("allocated_compaction_calls"),
					SimpleExpr::CustomWithExpr(
						"(allocated_compaction_calls - ?)".to_owned(),
						vec![Expr::value(query_bind_4.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("allocated_embedding_calls"),
					SimpleExpr::CustomWithExpr(
						"(allocated_embedding_calls - ?)".to_owned(),
						vec![Expr::value(query_bind_5.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant = ? AND id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;

		Ok(())
	}

	async fn mark_quota_released(&mut self, job: &Request) -> aidash_application::Result<()> {
		let tx = &mut *self.transaction;

		let query_bind_1 = job.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("generation_requests"))
				.value_expr(
					reinhardt::query::Alias::new("quota_released"),
					reinhardt::query::Expr::cust("TRUE"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;

		Ok(())
	}

	async fn cancel_runs(&mut self, job: &Request) -> aidash_application::Result<()> {
		let tx = &mut *self.transaction;
		let f = self.runtime;

		let query_bind_1 = job.task_id;
		let query_bind_2 = &job.home_node;
		let query_bind_3 = &f.config.node_id;
		sqlx::query(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("control"), reinhardt::query::Expr::cust("'CANCELLED'"))
				.and_where(SimpleExpr::CustomWithExpr("(task_id = ? AND (home_node = ? OR (?='' AND home_node=?)) AND NOT phase IN ('COMPLETED', 'FAILED', 'CANCELLED'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.execute(&mut **tx)
		.await.map_err(Error::from)?;

		Ok(())
	}

	async fn authority(&mut self, tenant: &str) -> aidash_application::Result<Snapshot> {
		crate::authorization::Authorization::load_with_mode(self.transaction, tenant, true)
			.await
			.map_err(Into::into)
	}

	async fn save_authority(
		&mut self,
		job: &Request,
		snapshot: &Snapshot,
		actor: &str,
	) -> aidash_application::Result<()> {
		let tx = &mut *self.transaction;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = snapshot.revision;
		let query_bind_3 = json!(snapshot.bundle);
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("authorization_bundles"))
				.value_expr(
					reinhardt::query::Alias::new("revision"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("document"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("updated_at"),
					reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = snapshot.revision;
		let query_bind_3 = json!(snapshot.bundle);
		let query_bind_4 = actor;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("authorization_revisions"))
				.columns([
					reinhardt::query::Alias::new("tenant"),
					reinhardt::query::Alias::new("revision"),
					reinhardt::query::Alias::new("document"),
					reinhardt::query::Alias::new("actor"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;

		Ok(())
	}

	async fn retire_catalog(&mut self, job: &Request) -> aidash_application::Result<Option<i64>> {
		let tx = &mut *self.transaction;
		Ok({
			let query_bind_1 = &job.tenant;
			let query_bind_2 = &job.agent_id;
			let query_bind_3 = &job.agent_version;
			sqlx::query_scalar(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("authorization_catalog"))
					.value_expr(
						reinhardt::query::Alias::new("enabled"),
						reinhardt::query::Expr::cust("FALSE"),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND entry_id = ? AND entry_version = ? AND enabled)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.returning_exprs([reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::Alias::new("revision")),
					)])
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await
			.map_err(Error::from)?
		})
	}

	async fn record_retirement(
		&mut self,
		job: &Request,
		revision: i64,
		actor: &str,
	) -> aidash_application::Result<()> {
		let tx = &mut *self.transaction;

		let query_bind_1 = job.id;
		let query_bind_2 = revision;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("generation_requests"))
				.value_expr(
					reinhardt::query::Alias::new("retired_catalog_revision"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = &job.agent_id;
		let query_bind_3 = &job.agent_version;
		let query_bind_4 = revision;
		let query_bind_5 = actor;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new(
					"authorization_catalog_history",
				))
				.columns([
					reinhardt::query::Alias::new("tenant"),
					reinhardt::query::Alias::new("entry_id"),
					reinhardt::query::Alias::new("entry_version"),
					reinhardt::query::Alias::new("revision"),
					reinhardt::query::Alias::new("enabled"),
					reinhardt::query::Alias::new("actor"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("FALSE"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_5.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;

		Ok(())
	}

	async fn update_status(
		&mut self,
		job: &Request,
		status: &str,
	) -> aidash_application::Result<Request> {
		let tx = &mut *self.transaction;
		Ok({
			let query_bind_1 = job.id;
			let query_bind_2 = status;
			crate::database::query_as(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("generation_requests"))
					.value_expr(
						reinhardt::query::Alias::new("status"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await
			.map_err(Error::from)?
		})
	}

	async fn history(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> aidash_application::Result<()> {
		let tx = &mut *self.transaction;

		let query_bind_1 = job.id;
		let query_bind_2 = status;
		let query_bind_3 = actor;
		let query_bind_4 = reason;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("generation_history"))
				.columns([
					reinhardt::query::Alias::new("request_id"),
					reinhardt::query::Alias::new("status"),
					reinhardt::query::Alias::new("actor"),
					reinhardt::query::Alias::new("reason"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await
		.map_err(Error::from)?;

		Ok(())
	}

	async fn event(
		&mut self,
		workspace: Uuid,
		kind: &str,
		data: Value,
	) -> aidash_application::Result<()> {
		self.runtime
			.store
			.event(self.transaction, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}

	async fn replay(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		input: &Control,
	) -> aidash_application::Result<bool> {
		let tx = &mut *self.transaction;
		let replay: bool = {
			let query_bind_1 = job.id;
			let query_bind_2 = status;
			let query_bind_3 = actor;
			let query_bind_4 = &input.reason;
			sqlx::query_scalar(&Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM generation_history WHERE request_id = ? AND status = ? AND actor = ? AND reason = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into()])).to_string(PostgresQueryBuilder)).fetch_one(&mut **tx).await.map_err(Error::from)?
		};

		Ok(replay)
	}

	async fn load(&mut self, tenant: &str, id: Uuid) -> aidash_application::Result<Request> {
		load(self.transaction, tenant, id).await.map_err(Into::into)
	}
}
pub(crate) async fn load(
	tx: &mut Transaction<'_, Postgres>,
	tenant: &str,
	id: Uuid,
) -> Result<Request> {
	{
		let query_bind_1 = tenant;
		let query_bind_2 = id;
		crate::database::query_as(
			&Query::select()
				.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
				.from(Alias::new("generation_requests"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant = ? AND id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **tx)
		.await?
	}
	.ok_or(Error::Forbidden)
}
