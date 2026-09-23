//! Reservations precede provider calls and survive process death. A successful
//! bounded usage report can refund unused tokens; unknown usage stays charged.
use crate::{
	Error, Result,
	authorization::access::Access,
	provider::{ModelRequest, ModelResponse},
	store::Store,
};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) struct Reservation {
	pool: PgPool,
	attempt: Uuid,
	requests: Vec<Uuid>,
	amount: i64,
}
impl Reservation {
	pub fn check_request(window: usize, request: &ModelRequest) -> Result<()> {
		let estimated = request.estimated_total_tokens();
		if estimated > window {
			return Err(Error::Invalid(format!(
				"model request exceeds context window: estimated total {estimated}, window {window}, output reserve {}, framing reserve 1024",
				request.max_output_tokens
			)));
		}
		Ok(())
	}
	pub async fn settle(self, response: &ModelResponse) -> Result<()> {
		let reported = response
			.input_tokens
			.checked_add(response.output_tokens)
			.and_then(|n| i64::try_from(n).ok());
		// Missing, overflowing or out-of-contract usage never frees allowance.
		let refund = reported
			.filter(|n| response.usage_complete && *n > 0 && *n <= self.amount)
			.map_or(0, |n| self.amount - n);
		let mut tx = self.pool.begin().await?;
		for id in self.requests {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("generation_budgets"))
					.value(
						sea_orm::sea_query::Alias::new("used_tokens"),
						sea_orm::sea_query::Expr::cust("used_tokens - $2"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("request_id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.bind(refund)
			.execute(&mut *tx)
			.await?;
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("generation_usage"))
					.value(
						sea_orm::sea_query::Alias::new("reported_tokens"),
						sea_orm::sea_query::Expr::cust("$3"),
					)
					.and_where(sea_orm::sea_query::Expr::cust(
						"request_id = $1 AND attempt_id = $2 AND reported_tokens IS NULL",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.bind(self.attempt)
			.bind(reported)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
		if reported.is_some_and(|n| n > self.amount) {
			return Err(Error::External(
				"provider usage exceeded reserved model limits".into(),
			));
		}
		Ok(())
	}
}

pub(crate) async fn reserve(
	access: &mut Access,
	store: &Store,
	run: Uuid,
	attempt: Uuid,
	window: usize,
	output: u32,
) -> Result<Option<Reservation>> {
	let requests: Vec<Uuid> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
			))
			.from(sea_orm::sea_query::Alias::new("generation_requests"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"tenant = $1 AND ($2 || '/agents/' || agent_id || '@' || agent_version) = ANY($3)",
			))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("id"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(&store.node_id)
	.bind(&access.subjects)
	.fetch_all(&mut **access.tx)
	.await?;
	if requests.is_empty() {
		return Ok(None);
	}
	let amount = window
		.checked_add(output as usize)
		.and_then(|n| i64::try_from(n).ok())
		.ok_or_else(|| Error::Invalid("model reservation overflow".into()))?;
	let mut tx = store.pool.begin().await?;
	for id in &requests {
		let reserved: Option<Uuid> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("generation_budgets"))
				.value(
					sea_orm::sea_query::Alias::new("used_tokens"),
					sea_orm::sea_query::Expr::cust("used_tokens + $2"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"request_id = $1 AND token_limit - used_tokens >= $2",
				))
				.returning(sea_orm::sea_query::Query::returning().exprs([
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("request_id"),
					)),
				]))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(amount)
		.fetch_optional(&mut *tx)
		.await?;
		if reserved.is_none() {
			return Err(Error::Invalid(
				"generated agent token budget exhausted".into(),
			));
		}
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("generation_usage"))
				.columns([
					sea_orm::sea_query::Alias::new("request_id"),
					sea_orm::sea_query::Alias::new("attempt_id"),
					sea_orm::sea_query::Alias::new("run_id"),
					sea_orm::sea_query::Alias::new("reserved_tokens"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
				])
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(attempt)
		.bind(run)
		.bind(amount)
		.execute(&mut *tx)
		.await?;
	}
	tx.commit().await?;
	Ok(Some(Reservation {
		pool: store.pool.clone(),
		attempt,
		requests,
		amount,
	}))
}
