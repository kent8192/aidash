//! Withdrawal rechecks committed rows under the original Area-before-operation locks.
use crate::{
	Error,
	apps::execution::capabilities::services::{
		contracts::Area,
		operations::{self, Operation},
		sessions,
	},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::withdrawal::{OperationWithdrawalRepository, OperationWithdrawalScope},
};
use aidash_domain::capabilities::operations::withdrawal::{Change, Snapshot};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) store: &'a Store,
}
struct Scope {
	tx: Transaction<'static, Postgres>,
	area: Area,
	operation: Operation,
}
#[async_trait]
impl OperationWithdrawalRepository for Repository<'_> {
	async fn lock(&self, area: Uuid, id: Uuid) -> Result<Box<dyn OperationWithdrawalScope>> {
		struct Seed {
			area_id: Uuid,
			id: Uuid,
		}
		let operation = Seed { area_id: area, id };
		let store = self.store;
		let mut tx = store.pool.begin().await.map_err(Error::from)?;
		// Keep the normal area-before-operation lock order and recheck committed
		// state: a stale worker snapshot cannot declare a dispatched writer safe.
		let area: Area = {
			let query_bind_1 = operation.area_id;
			sqlx::query_as(
				&sessions::select("core_areas")
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await
			.map_err(Error::from)?
		};
		let operation: Operation = {
			let query_bind_1 = operation.id;
			sqlx::query_as(
				&sessions::select("core_operations")
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await
			.map_err(Error::from)?
		};

		Ok(Box::new(Scope {
			tx,
			area,
			operation,
		}))
	}
	async fn cancel(&self, operation: Uuid) -> Result<Value> {
		operations::remote(
			self.store,
			reqwest::Method::POST,
			&format!("/v1/operations/{operation}/cancel"),
			None,
		)
		.await
		.map_err(Into::into)
	}
}
#[async_trait]
impl OperationWithdrawalScope for Scope {
	fn snapshot(&self) -> Snapshot {
		Snapshot {
			operation_id: self.operation.id,
			state: self.operation.state.clone(),
			epoch: self.operation.epoch,
			generation: self.operation.generation,
			area_epoch: self.area.epoch,
			area_generation: self.area.generation,
			area_state: self.area.state.clone(),
		}
	}
	async fn commit(self: Box<Self>, change: Change) -> Result<()> {
		let Self {
			mut tx, operation, ..
		} = *self;

		{
			let query_bind_1 = operation.id;
			let query_bind_2 = change.state;
			let query_bind_3 = change.result;
			sqlx::query(
				&Query::update()
					.table(Alias::new("core_operations"))
					.value_expr(
						Alias::new("state"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("result"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
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
			.execute(&mut *tx)
			.await
			.map_err(Error::from)?
		};
		if let Some(area_state) = change.area_state {
			{
				let query_bind_1 = operation.area_id;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_areas"))
						.value_expr(Alias::new("state"), Expr::val(area_state))
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await
				.map_err(Error::from)?
			};
		}
		tx.commit().await.map_err(Error::from)?;
		Ok(())
	}
}
