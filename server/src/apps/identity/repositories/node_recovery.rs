//! Current registration dependencies of immutable, replayable atomic journals.
//! This is recovery state, not audit retention or permission to admit fresh work.
use crate::{Error, Result};
use aidash_domain::{
	nodes::{DeploymentId, NodeAddress, NodeName},
	transactions::Manifest,
};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{QueryRow, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use std::collections::BTreeSet;
use uuid::Uuid;

fn blocked() -> Error {
	Error::Conflict("Node purge cannot verify an atomic recovery registration dependency; retain its current trust and inspect the recovery journal".into())
}

/// Exact counterpart addresses for a permanent local owner. The caller holds
/// its policy/capacity locks; destructive purge additionally holds the atomic
/// gate in this same transaction. No Node catalog or deletion-history lookup is
/// used, so this also supports administration of an existing retained row.
/// Missing required current trust is an integrity failure, never synthesized.
/// The conservative global journal read is unbounded and keeps those rows
/// shared-locked until this transaction ends; file batch size does not bound it.
pub async fn registration_sources_in(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	tenant: &str,
	node: &str,
) -> Result<Vec<String>> {
	let name = NodeName::parse(node).map_err(|_| blocked())?;
	let owner = deployment.node(name);
	let mut sources = BTreeSet::new();
	for table in ["atomic_coordinators", "atomic_participants"] {
		let mut query = Query::select();
		query.columns(["id", "digest", "manifest"].map(Alias::new));
		if table == "atomic_participants" {
			query.column(Alias::new("coordinator"));
		}
		let (sql, values) = query
			.from(Alias::new(table))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		for row in tx.fetch_all(&sql, convert_values(values)).await? {
			let id: Uuid = row
				.get("id")
				.map_err(reinhardt::core::exception::Error::from)?;
			let digest: String = row
				.get("digest")
				.map_err(reinhardt::core::exception::Error::from)?;
			let stored_coordinator = if table == "atomic_participants" {
				Some(
					row.get::<String>("coordinator")
						.map_err(reinhardt::core::exception::Error::from)?,
				)
			} else {
				None
			};
			let manifest: Manifest =
				serde_json::from_value(QueryRow::from_backend_row(row).data["manifest"].clone())
					.map_err(|_| blocked())?;
			manifest.validate::<Error>().map_err(|_| blocked())?;
			if manifest.id != id || manifest.digest().map_err(|_| blocked())? != digest {
				return Err(blocked());
			}
			let coordinator = NodeAddress::parse(&manifest.coordinator).map_err(|_| blocked())?;
			let participants = manifest
				.participants
				.iter()
				.map(|participant| NodeAddress::parse(&participant.node_id).map_err(|_| blocked()))
				.collect::<Result<Vec<_>>>()?;
			let local = participants
				.iter()
				.find(|participant| &participant.deployment == deployment)
				.ok_or_else(blocked)?;
			// Permanent names are Deployment-wide identity, not current liveness.
			// Validate the local journal owner even if its catalog/audit expired.
			let holder = super::node_registrations::name_tenant(tx, local.node.as_str()).await?;
			if holder.is_none() || (local == &owner && holder.as_deref() != Some(tenant)) {
				return Err(blocked());
			}
			if table == "atomic_coordinators" {
				if &coordinator.deployment != deployment || local != &coordinator {
					return Err(blocked());
				}
				if coordinator == owner {
					sources.extend(
						participants
							.iter()
							.filter(|participant| &participant.deployment != deployment)
							.map(ToString::to_string),
					);
				}
			} else {
				if stored_coordinator.as_deref() != Some(manifest.coordinator.as_str()) {
					return Err(blocked());
				}
				if local == &owner && &coordinator.deployment != deployment {
					sources.insert(coordinator.to_string());
				}
			}
		}
	}
	if sources.is_empty() {
		return Ok(Vec::new());
	}
	let (sql, values) = Query::select()
		.expr(Expr::value(1_i32))
		.from(Alias::new("node_names"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("name").eq(Expr::value(node)))
		.lock(LockType::KeyShare)
		.build(PostgresQueryBuilder);
	if tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.is_none()
	{
		return Err(blocked());
	}
	for source in &sources {
		let (sql, values) = Query::select()
			.expr(Expr::value(1_i32))
			.from(Alias::new("node_registrations"))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("node").eq(Expr::value(node)))
			.and_where(Expr::col("source_node").eq(Expr::value(source.clone())))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_none()
		{
			return Err(blocked());
		}
	}
	Ok(sources.into_iter().collect())
}
