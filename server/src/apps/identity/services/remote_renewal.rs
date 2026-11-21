//! The Home renews Remote Grants while their Runs live (ADR 0109, amended
//! 2026-11-17). The decision lives in
//! `aidash_application::authorization::source::renewal`; this file reads and
//! writes the grant rows and re-evaluates the delegation chain.
//!
//! Renewal runs in two places. A watched loop looks, every few minutes, at the
//! grants that expire within the renewal window while their Run lives by the
//! Home's one liveness rule ([`run_live`]: a recorded activation and a Task that
//! is still waiting for its first claim or held by the receiver), so a Run that waits for a
//! person's answer without making calls is renewed too, and a grant whose
//! activation was never recorded is not. Serving a description or verification of such a grant
//! renews it as well. Both extend the expiry only from the value they read, so
//! replicas and the two paths together renew a grant once per window.
use crate::Result;
use crate::apps::identity::repositories::remote_grants::{clock_timestamp, run_live, unexpired};
use crate::federation::Federation;
use aidash_application::authorization::source::renewal::{
	GrantLife, RENEW_WITHIN, RENEWAL_BATCH, Renewal, RenewalRepository, renew, renew_after_use,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

const DISCOVERY: TenancyScope = TenancyScope::Deployment(DeploymentScopeReason::RemoteGrantRenewal);

/// `CLOCK_TIMESTAMP() + make_interval(0, 0, 0, 0, 0, 0, seconds)`: `seconds` from now by the
/// clock. The positional form is `make_interval(secs => seconds)`; a function call of the query
/// builder takes no named arguments.
fn seconds_from_now(seconds: i64) -> SimpleExpr {
	let zero = || Expr::value(0_i32).into();
	clock_timestamp().add(SimpleExpr::FunctionCall(
		"make_interval".into_iden(),
		vec![
			zero(),
			zero(),
			zero(),
			zero(),
			zero(),
			zero(),
			Expr::value(seconds as f64).into(),
		],
	))
}

/// What one pass of the loop did.
#[derive(Debug, Default)]
pub struct Pass {
	pub renewed: usize,
	pub chain_broken: usize,
	pub failed: usize,
}

/// Renews the Remote Grants near expiry whose Runs live. A grant that fails is
/// logged and left for the next pass, which still falls inside the window.
pub async fn sweep(f: &Federation) -> Result<Pass> {
	let window = RENEW_WITHIN.num_seconds();
	let candidates = f
		.store
		.pool
		.atomic(&DISCOVERY, async |tx| {
			let (sql, values) = Query::select()
				.column(Alias::new("tenant"))
				.column(Alias::new("id"))
				.column(Alias::new("node_id"))
				.from(Alias::new("authorization_remote_grants"))
				.and_where(unexpired())
				.and_where(Expr::col(Alias::new("expires_at")).lte(seconds_from_now(window)))
				.and_where(run_live())
				.order_by(Alias::new("expires_at"), Order::Asc)
				.limit(RENEWAL_BATCH)
				.build(PostgresQueryBuilder);
			tx.fetch_all(&sql, convert_values(values))
				.await?
				.into_iter()
				.map(|row| {
					Ok((
						row.get::<String>("tenant")
							.map_err(reinhardt::core::exception::Error::from)?,
						row.get::<Uuid>("id")
							.map_err(reinhardt::core::exception::Error::from)?,
						row.get::<String>("node_id")
							.map_err(reinhardt::core::exception::Error::from)?,
					))
				})
				.collect::<Result<Vec<_>>>()
		})
		.await?;
	let mut pass = Pass::default();
	for (tenant, id, node_id) in candidates {
		let repository = Grants {
			federation: f.clone(),
			tenant,
			node_id,
		};
		match renew(&repository, id).await {
			Ok(Renewal::Renewed) => pass.renewed += 1,
			Ok(Renewal::ChainBroken) => pass.chain_broken += 1,
			Ok(_) => {}
			Err(error) => {
				pass.failed += 1;
				tracing::warn!(%error, grant = %id, "Remote Grant renewal failed");
			}
		}
	}
	Ok(pass)
}

/// Renews a grant the Home has just described or verified, which re-evaluated
/// its chain. Never fails the caller: a grant that is not renewed expires as it
/// would have.
pub async fn after_use(f: &Federation, tenant: &str, node_id: &str, id: Uuid) {
	let repository = Grants {
		federation: f.clone(),
		tenant: tenant.to_owned(),
		node_id: node_id.to_owned(),
	};
	if let Err(error) = renew_after_use(&repository, id).await {
		tracing::warn!(%error, grant = %id, "Remote Grant renewal on use failed");
	}
}

struct Grants {
	federation: Federation,
	tenant: String,
	/// The receiving Node as the grant records it, which the description path
	/// matches against the caller.
	node_id: String,
}

#[async_trait]
impl RenewalRepository for Grants {
	async fn life(&self, id: Uuid) -> aidash_application::Result<Option<GrantLife>> {
		let scope = TenancyScope::tenant(&self.tenant)?;
		self.federation
			.store
			.pool
			.atomic(&scope, async |tx| {
				let (sql, values) = Query::select()
					.column(Alias::new("revoked"))
					.column(Alias::new("expires_at"))
					.expr_as(clock_timestamp(), Alias::new("now"))
					.expr_as(run_live(), Alias::new("run_live"))
					.from(Alias::new("authorization_remote_grants"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.build(PostgresQueryBuilder);
				tx.fetch_all(&sql, convert_values(values))
					.await?
					.into_iter()
					.next()
					.map(|row| {
						Ok(GrantLife {
							revoked: row
								.get::<bool>("revoked")
								.map_err(reinhardt::core::exception::Error::from)?,
							expires_at: row
								.get::<DateTime<Utc>>("expires_at")
								.map_err(reinhardt::core::exception::Error::from)?,
							now: row
								.get::<DateTime<Utc>>("now")
								.map_err(reinhardt::core::exception::Error::from)?,
							run_live: row
								.get::<bool>("run_live")
								.map_err(reinhardt::core::exception::Error::from)?,
						})
					})
					.transpose()
			})
			.await
			.map_err(Into::into)
	}

	/// Serves a description the way the receiver's own call would, which runs
	/// the source checks of preparation against the saved chain. Any refusal,
	/// and any failure to reach the receiver, leaves the grant unrenewed.
	async fn chain_holds(&self, id: Uuid) -> aidash_application::Result<bool> {
		match crate::authorization::remote::description_lease(&self.federation, &self.node_id, id)
			.await
		{
			Ok((access, _)) => {
				access.finish(Ok(())).await?;
				Ok(true)
			}
			Err(error) => {
				tracing::debug!(%error, grant = %id, "Remote Grant chain not confirmed");
				Ok(false)
			}
		}
	}

	async fn extend(
		&self,
		id: Uuid,
		seen: DateTime<Utc>,
		ttl_seconds: i64,
	) -> aidash_application::Result<bool> {
		let scope = TenancyScope::tenant(&self.tenant)?;
		self.federation
			.store
			.pool
			.atomic(&scope, async |tx| {
				// Conditioned on the expiry that was read, so of several replicas
				// (or the loop and a description) only one changes the row.
				let (sql, values) = Query::update()
					.table(Alias::new("authorization_remote_grants"))
					.value_expr(Alias::new("expires_at"), seconds_from_now(ttl_seconds))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.and_where(Expr::col("expires_at").eq(Expr::value(seen)))
					.and_where(unexpired())
					.and_where(run_live())
					.build(PostgresQueryBuilder);
				Ok(tx
					.execute(&sql, convert_values(values))
					.await?
					.rows_affected == 1)
			})
			.await
			.map_err(Into::into)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// `expires_at <= CLOCK_TIMESTAMP() + make_interval(secs => 300)`, built by the query builder:
	/// `make_interval`'s six leading arguments are zero, and the seconds are double precision.
	#[test]
	fn the_renewal_window_is_built_without_raw_sql() {
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("authorization_remote_grants"))
			.and_where(Expr::col(Alias::new("expires_at")).lte(seconds_from_now(300)))
			.build(PostgresQueryBuilder);
		assert_eq!(
			sql,
			"SELECT \"id\" FROM \"authorization_remote_grants\" WHERE \"expires_at\" <= \
			CLOCK_TIMESTAMP() + make_interval($1, $2, $3, $4, $5, $6, $7)"
		);
		assert_eq!(
			format!("{values:?}"),
			format!(
				"{:?}",
				reinhardt::query::Values(vec![
					0_i32.into(),
					0_i32.into(),
					0_i32.into(),
					0_i32.into(),
					0_i32.into(),
					0_i32.into(),
					300_f64.into(),
				])
			)
		);
	}
}
