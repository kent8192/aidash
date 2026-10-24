//! Resumable, fenced Node erasure (ADR 0083). Discovery is read-only across
//! Tenants; each destructive transaction uses exactly the owning Tenant.
//!
//! Files drain in batches of 100. The final rows step is deliberately atomic
//! and unbounded: it locks a transaction-local CTID snapshot before deleting
//! anchors. Subject/Node admission closure must drain writers before this runs.
use crate::apps::identity::models::{AuthorizationBundle, NodeDeletion, database_time};
use crate::apps::identity::repositories::{node_purge as progress, nodes};
use crate::database::{classification, native, purge};
use crate::store::Store;
use crate::{Error, Result};
use aidash_application::authorization::Snapshot;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::nodes::deletion::PurgeStep;
use aidash_domain::policy::SubjectKind;
use chrono::Duration;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;
mod objects;
mod preflight;
mod roles;
mod rows;
mod selectors;
use selectors::Owners;
const DISCOVERY: TenancyScope = TenancyScope::Deployment(DeploymentScopeReason::NodePurge);
const CLAIM_BATCH: u64 = 20;
const OBJECT_BATCH: u64 = 100;

/// Runs due/requested purges and expires their separable management audit.
/// A stalled Node does not prevent other candidates from making progress.
pub async fn sweep(store: &Store, audit_retention_days: u32) -> Result<()> {
	for deletion in claim(store).await? {
		let tenant = deletion.tenant.clone();
		let node = deletion.node.clone();
		if let Err(error) = purge_node(store, deletion).await {
			tracing::warn!(%tenant,%node,%error,"Node purge stalled; retry after its blocker is resolved");
		}
	}
	erase_audit(store, audit_retention_days).await
}

/// Discovers candidates without writing across Tenants, then claims each one
/// under policy -> capacity -> deletion locks. Claim sets FIRST before any I/O.
pub async fn claim(store: &Store) -> Result<Vec<NodeDeletion>> {
	let candidates = store
		.pool
		.atomic(&DISCOVERY, async |tx| {
			rows::coverage(tx).await?;
			let now = database_time(tx).await?;
			let blocked = Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("tenant_deletions"))
				.and_where(Expr::col("restored_at").is_null())
				.to_owned();
			let (sql, values) = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("node_deletions"))
				.and_where(Expr::col("restored_at").is_null())
				.and_where(Expr::col("purged_at").is_null())
				.and_where(
					Expr::col("purge_after")
						.lte(Expr::value(now))
						.or(Expr::col("purge_step").is_not_null()),
				)
				.and_where(
					Expr::col("purge_leased_until")
						.is_null()
						.or(Expr::col("purge_leased_until").lte(Expr::value(now))),
				)
				.and_where(Expr::col("tenant").not_in_subquery(blocked))
				.order_by_expr(Expr::col("purge_failed_at").is_null(), Order::Desc)
				.order_by(Alias::new("purge_failed_at"), Order::Asc)
				.order_by(Alias::new("purge_after"), Order::Asc)
				.limit(CLAIM_BATCH)
				.build(PostgresQueryBuilder);
			let ids = tx
				.fetch_all(&sql, convert_values(values))
				.await?
				.into_iter()
				.map(|r| {
					r.get::<Uuid>("id")
						.map_err(reinhardt::core::exception::Error::from)
				})
				.collect::<std::result::Result<Vec<_>, _>>()?;
			Ok(NodeDeletion::objects()
				.filter(NodeDeletion::field_id().is_in(ids))
				.all_with_executor(tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?)
		})
		.await?;
	let mut claimed = Vec::new();
	for candidate in candidates {
		let scope = TenancyScope::tenant(&candidate.tenant)?;
		let result = store
			.pool
			.atomic(&scope, async |tx| {
				AuthorizationBundle::lock_snapshot(tx, &candidate.tenant, true).await?;
				nodes::lock_tenant(tx, &candidate.tenant).await?;
				progress::claim_one(tx, &candidate).await
			})
			.await;
		match result {
			Ok(Some(row)) => claimed.push(row),
			Ok(None) | Err(Error::TenantDeleted) => {}
			Err(error) => return Err(error),
		}
	}
	Ok(claimed)
}

/// Drives a claimed deletion. Stale, restored, expired or replaced leases may
/// neither erase data nor overwrite the current worker's failure/progress.
pub async fn purge_node(store: &Store, deletion: NodeDeletion) -> Result<()> {
	let mut step = deletion
		.purge_step
		.as_deref()
		.map(PurgeStep::parse)
		.transpose()?
		.unwrap_or(PurgeStep::FIRST);
	loop {
		if let Err(error) = run_step(store, &deletion, step).await {
			// Errors from the filesystem/DB may carry private values. Persist only a
			// known blocker message or a generic retry message, never those details.
			let message = match &error {
				Error::Conflict(reason) if reason.starts_with("Node purge ") => reason.as_str(),
				_ => "Node purge could not complete; inspect the worker and retry",
			};
			let scope = TenancyScope::tenant(&deletion.tenant)?;
			let _ = store
				.pool
				.atomic(&scope, async |tx| {
					AuthorizationBundle::lock_snapshot(tx, &deletion.tenant, true).await?;
					nodes::lock_tenant(tx, &deletion.tenant).await?;
					progress::hold(tx, &deletion).await?;
					progress::fail(tx, &deletion, step, message).await
				})
				.await;
			return Err(error);
		}
		let Some(next) = step.next() else {
			return Ok(());
		};
		step = next;
	}
}

/// Executes one production step; exposed for maintenance/manual recovery and
/// deterministic crash/inter-step integration tests. It never bypasses fencing.
pub async fn run_step(store: &Store, deletion: &NodeDeletion, step: PurgeStep) -> Result<()> {
	loop {
		let mut tx = store
			.pool
			.begin(&TenancyScope::tenant(&deletion.tenant)?)
			.await?;
		let mut snapshot =
			AuthorizationBundle::lock_snapshot(&mut *tx, &deletion.tenant, true).await?;
		nodes::lock_tenant(&mut *tx, &deletion.tenant).await?;
		if progress::hold(&mut *tx, deletion).await?.as_deref() != Some(step.as_str()) {
			return Err(Error::Conflict(
				"Node purge step changed; reload its progress before retrying".into(),
			));
		}
		crate::transactions::gate::read_in(&mut tx).await?;
		let mut owners = owners(deletion, &snapshot);
		roles::resolve(&mut *tx, store, &mut owners).await?;
		// Prospective policy and all cross-owner references are checked before the
		// first file unlink, not merely when a later FK DELETE happens to fail.
		preflight::check(&mut *tx, &owners, &snapshot).await?;
		owners.retained_registrations =
			crate::apps::identity::repositories::node_recovery::registration_sources_in(
				&mut *tx,
				&store.deployment,
				owners.tenant,
				owners.node,
			)
			.await?;
		rows::validate(&mut *tx, &owners).await?;
		let complete = match step {
			PurgeStep::CapabilityObjects => objects::erase(store, &mut tx, &owners).await?,
			PurgeStep::Rows => {
				if !objects::ids(&mut *tx, &owners, 1).await?.is_empty() {
					return Err(Error::Conflict("Node purge found new files after its file step; close outstanding writers before recovery".into()));
				}
				rows::erase(&mut *tx, &owners).await?;
				snapshot.bundle.subjects.retain(|_, s| {
					s.kind == SubjectKind::User
						|| s.node
							.as_ref()
							.is_none_or(|node| node.as_str() != deletion.node)
				});
				AuthorizationBundle::replace(
					&mut *tx,
					&deletion.tenant,
					snapshot.revision,
					serde_json::to_value(&snapshot.bundle)?,
					"node-purge",
					None,
				)
				.await?;
				true
			}
			PurgeStep::Finish => {
				progress::finish(&mut *tx, deletion).await?;
				true
			}
		};
		if step != PurgeStep::Finish {
			if complete {
				progress::advance(
					&mut *tx,
					deletion,
					step.next().expect("a nonfinal step has a successor"),
				)
				.await?;
			} else {
				progress::renew(&mut *tx, deletion).await?;
			}
		}
		tx.commit().await?;
		if complete {
			return Ok(());
		}
	}
}
fn owners<'a>(deletion: &'a NodeDeletion, snapshot: &Snapshot) -> Owners<'a> {
	Owners {
		journals: Vec::new(),
		home_journals: Vec::new(),
		remote_usage: Vec::new(),
		dispatches: Vec::new(),
		retained_registrations: Vec::new(),
		tenant: &deletion.tenant,
		node: &deletion.node,
		subjects: snapshot
			.bundle
			.subjects
			.iter()
			.filter(|(_, s)| {
				s.kind != SubjectKind::User
					&& s.node.as_ref().is_some_and(|n| n.as_str() == deletion.node)
			})
			.map(|(name, _)| name.clone())
			.collect(),
	}
}

/// At the final purge's audit deadline, erase every deletion cycle for this
/// Node, including earlier restores. Only node_names remains permanently.
pub async fn erase_audit(store: &Store, retention_days: u32) -> Result<()> {
	let due = store
		.pool
		.atomic(&DISCOVERY, async |tx| {
			let now = database_time(tx).await?;
			Ok(NodeDeletion::objects()
				.filter(
					NodeDeletion::field_purged_at()
						.lte(Some(now - Duration::days(i64::from(retention_days)))),
				)
				.order_by(&["purged_at", "id"])
				.limit(CLAIM_BATCH as usize)
				.all_with_executor(tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?)
		})
		.await?;
	for deletion in due {
		store
			.pool
			.atomic(&TenancyScope::tenant(&deletion.tenant)?, async |tx| {
				// Keep management response snapshots and expiry serialized. Never
				// remove the current deletion while its catalog could appear live.
				AuthorizationBundle::lock_frozen(tx, &deletion.tenant, true).await?;
				nodes::lock_tenant(tx, &deletion.tenant).await?;
				rows::coverage(tx).await?;
				let now = database_time(tx).await?;
				let (sql, values) = Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("node_deletions"))
					.and_where(Expr::col("id").eq(Expr::value(deletion.id)))
					.and_where(
						Expr::col("purged_at")
							.lte(Expr::value(now - Duration::days(i64::from(retention_days)))),
					)
					.build(PostgresQueryBuilder);
				if tx
					.fetch_optional(&sql, convert_values(values))
					.await?
					.is_none()
				{
					return Ok(());
				}
				// Final purge is immutable and Node names are never reused. Concurrent
				// expiry is idempotent; delete in audit-table order instead of locking
				// the last table first and inverting whole-Tenant purge's lock order.
				// Task2 provides the explicit nullable administration-history.node column.
				for (table, key) in [
					("nodes", "name"),
					("authorization_catalog_history", "node"),
					("dashboard_administration_history", "node"),
					("node_deletions", "node"),
				] {
					let (sql, values) = Query::delete()
						.from_table(Alias::new(table))
						.and_where(Expr::col("tenant").eq(Expr::value(&deletion.tenant)))
						.and_where(Expr::col(key).eq(Expr::value(&deletion.node)))
						.build(PostgresQueryBuilder);
					tx.execute(&sql, convert_values(values)).await?;
				}
				Ok(())
			})
			.await?;
	}
	Ok(())
}

/// Exact retained file bytes for an already-authorized management response.
/// Call only for a bounded management page/lifecycle response, never the hot
/// ordinary Node selector. This is storage footprint, not a billing projection.
/// Store wrapper for independent reads; lifecycle/list callers must reuse their
/// existing transaction through [`retained_storage_bytes_in`].
pub async fn retained_storage_bytes(store: &Store, tenant: &str, node: &str) -> Result<u64> {
	store
		.pool
		.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
			retained_storage_bytes_in(tx, tenant, node).await
		})
		.await
}

/// Same-transaction management projection. The caller authorizes management
/// access before calling; reentrant policy reads do not open a second pool
/// connection or see a different lifecycle snapshot.
pub async fn retained_storage_bytes_in(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<u64> {
	use reinhardt::query::{IntoIden, SimpleExpr};
	let snapshot = AuthorizationBundle::lock_frozen(tx, tenant, false).await?;
	let owners = Owners {
		journals: Vec::new(),
		home_journals: Vec::new(),
		remote_usage: Vec::new(),
		dispatches: Vec::new(),
		retained_registrations: Vec::new(),
		tenant,
		node,
		subjects: snapshot
			.bundle
			.subjects
			.iter()
			.filter(|(_, s)| {
				s.kind != SubjectKind::User && s.node.as_ref().is_some_and(|n| n.as_str() == node)
			})
			.map(|(name, _)| name.clone())
			.collect(),
	};
	let provisional = objects::provisional(tx, &owners).await?;
	let mut total = 0u64;
	for (table, field, function) in [
		("core_objects", "size", false),
		("channel_attachments", "size_bytes", false),
		("artifacts", "content", true),
		("research_snapshots", "text_bytes", false),
		("research_originals", "bytes", true),
		("research_page_images", "bytes", true),
	] {
		let mut size = selectors::col(table, field);
		if table == "artifacts" {
			size = size.cast_as(Alias::new("text"));
		}
		if function {
			size = SimpleExpr::FunctionCall("octet_length".into_iden(), vec![size]);
		}
		let sum = SimpleExpr::FunctionCall("sum".into_iden(), vec![size]);
		let sum =
			SimpleExpr::FunctionCall("coalesce".into_iden(), vec![sum, Expr::value(0i64).into()])
				.cast_as(Alias::new("int8"));
		let mut predicate = owners.predicate(table)?;
		if table == "core_objects" {
			predicate = predicate.or(Expr::col("tenant")
				.eq(Expr::value(tenant))
				.and(Expr::col("area_id").is_null())
				.and(selectors::ids(
					Expr::col("id").into(),
					&provisional.iter().copied().collect::<Vec<_>>(),
				)));
		}
		let (sql, values) = Query::select()
			.expr_as(sum, Alias::new("bytes"))
			.from(Alias::new(table))
			.and_where(predicate)
			.build(PostgresQueryBuilder);
		let bytes: i64 = tx
			.fetch_one(&sql, convert_values(values))
			.await?
			.get("bytes")
			.map_err(reinhardt::core::exception::Error::from)?;
		total = total
			.checked_add(
				u64::try_from(bytes)
					.map_err(|_| Error::Invalid("negative Node storage size".into()))?,
			)
			.ok_or_else(|| Error::Invalid("Node storage size overflow".into()))?;
	}
	Ok(total)
}
