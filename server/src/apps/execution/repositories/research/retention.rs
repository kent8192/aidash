//! The retention sweep of Document snapshot contents (ADR 0045), run in
//! Deployment Scope on the hourly retention loop.
use super::native;
use crate::apps::execution::models::research::{
	ResearchOriginal, ResearchSnapshot, revoke_invocations,
};
use crate::store::Store;
use aidash_application::{
	Result,
	ports::research::{RetainedSnapshot, SnapshotRetention},
};
use aidash_domain::retention::{RetentionRules, SnapshotPeriods};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, JoinType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SelectStatement, SimpleExpr, TableRef,
};
use serde::Deserialize;
use uuid::Uuid;

pub(crate) struct Snapshots {
	pub store: Store,
}

#[derive(Deserialize)]
struct Candidate {
	id: Uuid,
	fetched_at: DateTime<Utc>,
	run_ended_at: Option<DateTime<Utc>>,
	snapshot_after_run_seconds: Option<i64>,
	snapshot_max_age_seconds: Option<i64>,
}

/// The Run has ended; terminal Runs never change phase again.
fn ended() -> SimpleExpr {
	Expr::col((Alias::new("r"), Alias::new("phase"))).is_in(["COMPLETED", "FAILED", "CANCELLED"])
}

fn snapshot(column: &str) -> (Alias, Alias) {
	(Alias::new("s"), Alias::new(column))
}

/// Unpurged snapshots with their Run's end and their Tenant's stored periods.
fn retained() -> SelectStatement {
	let mut query = Query::select();
	query
		.column(snapshot("id"))
		.column(snapshot("fetched_at"))
		// A terminal Run is saved for the last time when it ends.
		.expr_as(
			Expr::case()
				.when(ended(), Expr::col((Alias::new("r"), Alias::new("updated_at"))))
				.else_result(Expr::null()),
			Alias::new("run_ended_at"),
		)
		.column((Alias::new("t"), Alias::new("snapshot_after_run_seconds")))
		.column((Alias::new("t"), Alias::new("snapshot_max_age_seconds")))
		.from_as(
			Alias::new(<ResearchSnapshot as reinhardt::db::orm::Model>::table_name()),
			Alias::new("s"),
		)
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("runs"), Alias::new("r")),
			Expr::col((Alias::new("r"), Alias::new("id"))).equals(snapshot("run_id")),
		)
		.join(
			JoinType::LeftJoin,
			TableRef::table_alias(Alias::new("authorization_workspaces"), Alias::new("w")),
			Expr::col((Alias::new("w"), Alias::new("workspace_id")))
				.equals((Alias::new("r"), Alias::new("workspace_id"))),
		)
		.join(
			JoinType::LeftJoin,
			TableRef::table_alias(Alias::new("tenant_retention_rules"), Alias::new("t")),
			Expr::col((Alias::new("t"), Alias::new("tenant")))
				.equals((Alias::new("w"), Alias::new("tenant"))),
		)
		.and_where(Expr::col(snapshot("purged_at")).is_null());
	query
}

async fn fetch_retained(
	tx: &mut dyn TransactionExecutor,
	query: &SelectStatement,
) -> crate::Result<Vec<RetainedSnapshot>> {
	let (sql, values) = query.build(PostgresQueryBuilder);
	let rows = tx.fetch_all(&sql, convert_values(values)).await?;
	rows.into_iter()
		.map(|row| {
			let candidate: Candidate =
				serde_json::from_value(QueryRow::from_backend_row(row).data)?;
			let period =
				|seconds: Option<i64>| seconds.and_then(|seconds| u32::try_from(seconds).ok());
			Ok(RetainedSnapshot {
				id: candidate.id,
				fetched_at: candidate.fetched_at,
				run_ended_at: candidate.run_ended_at,
				tenant_rules: period(candidate.snapshot_after_run_seconds)
					.zip(period(candidate.snapshot_max_age_seconds))
					.map(|(after_run_seconds, max_age_seconds)| RetentionRules {
						document_snapshots: SnapshotPeriods {
							after_run_seconds,
							max_age_seconds,
						},
					}),
			})
		})
		.collect()
}

#[async_trait]
impl SnapshotRetention for Snapshots {
	async fn candidates(
		&self,
		after: Option<Uuid>,
		retrieved_before: DateTime<Utc>,
		ended_before: DateTime<Utc>,
		limit: u32,
	) -> Result<Vec<RetainedSnapshot>> {
		let mut query = retained();
		query
			// A Run that ended within the shortest post-run period, or a
			// snapshot younger than the shortest maximum age, keeps its basis.
			.and_where(
				ended()
					.and(Expr::col((Alias::new("r"), Alias::new("updated_at"))).lt(Expr::value(ended_before)))
					.or(Expr::col(snapshot("fetched_at")).lt(Expr::value(retrieved_before))),
			)
			.order_by(snapshot("id"), Order::Asc)
			.limit(u64::from(limit));
		if let Some(after) = after {
			query.and_where(Expr::col(snapshot("id")).gt(Expr::value(after)));
		}
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let candidates = fetch_retained(tx.as_mut(), &query).await?;
				tx.commit().await?;
				Ok(candidates)
			}
			.await,
		)
	}
	async fn shortest_tenant_periods(&self) -> Result<Option<SnapshotPeriods>> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::min(Expr::col(Alias::new("snapshot_after_run_seconds")).into()),
				Alias::new("after_run"),
			)
			.expr_as(
				Func::min(Expr::col(Alias::new("snapshot_max_age_seconds")).into()),
				Alias::new("max_age"),
			)
			.from(Alias::new("tenant_retention_rules"))
			.build(PostgresQueryBuilder);
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let row = tx.fetch_one(&sql, convert_values(values)).await?;
				tx.commit().await?;
				let data = QueryRow::from_backend_row(row).data;
				let seconds = |value: &serde_json::Value| {
					value
						.as_i64()
						.or_else(|| value.as_str().and_then(|text| text.parse().ok()))
						.and_then(|seconds| u32::try_from(seconds).ok())
				};
				Ok(seconds(&data["after_run"])
					.zip(seconds(&data["max_age"]))
					.map(|(after_run_seconds, max_age_seconds)| SnapshotPeriods {
						after_run_seconds,
						max_age_seconds,
					}))
			}
			.await,
		)
	}
	async fn purge(
		&self,
		ids: &[Uuid],
		expired: &(dyn for<'a> Fn(&'a RetainedSnapshot) -> bool + Sync),
		at: DateTime<Utc>,
	) -> Result<u64> {
		if ids.is_empty() {
			return Ok(0);
		}
		let mut query = retained();
		query
			.expr_as(
				Expr::col((Alias::new("w"), Alias::new("tenant"))),
				Alias::new("tenant"),
			)
			.and_where(Expr::col(snapshot("id")).is_in(ids.iter().map(|id| Expr::value(*id))));
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				// A change of a Tenant's periods holds its lock exclusively, so
				// the periods read below stay current until the purge commits.
				let (sql, values) = query.build(PostgresQueryBuilder);
				let tenants = tx
					.fetch_all(&sql, convert_values(values))
					.await?
					.into_iter()
					.filter_map(|row| {
						QueryRow::from_backend_row(row).data["tenant"]
							.as_str()
							.map(str::to_owned)
					})
					.collect::<std::collections::BTreeSet<_>>();
				for tenant in &tenants {
					crate::apps::identity::repositories::retention::share_rules_lock(
						tx.as_mut(),
						tenant,
					)
					.await?;
				}
				let still = fetch_retained(tx.as_mut(), &query)
					.await?
					.into_iter()
					.filter(|candidate| expired(candidate))
					.map(|candidate| candidate.id)
					.collect::<Vec<_>>();
				let purged = ResearchSnapshot::purge(tx.as_mut(), &still, at).await?;
				// An original is part of its snapshot's contents (ADR 0053).
				ResearchOriginal::purge(tx.as_mut(), &still).await?;
				tx.commit().await?;
				Ok(purged)
			}
			.await,
		)
	}
}

/// One retention pass: purge every snapshot without a retention basis, and
/// catch up revoked Runs whose stored copies were written while they were
/// being revoked: tombstone research results stored for their invocations
/// and scrub their stored projections. Returns how many snapshots were
/// purged.
pub(crate) async fn sweep(store: &Store, rules: &RetentionRules) -> Result<u64> {
	let purged = aidash_application::research::retention::sweep(
		&Snapshots {
			store: store.clone(),
		},
		rules,
		Utc::now(),
		aidash_application::research::retention::BATCH,
	)
	.await?;
	native(
		async {
			let mut tx = store.database().begin().await?;
			revoke_invocations(tx.as_mut(), None).await?;
			tx.commit().await?;
			Ok(())
		}
		.await,
	)?;
	super::scrub_revoked_projections(store).await?;
	Ok(purged)
}

#[cfg(test)]
mod tests;
