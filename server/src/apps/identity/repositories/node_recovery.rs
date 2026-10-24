//! Exact current trust dependencies of retained atomic journals and terminal receipts.
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
	Error::Conflict("Node purge cannot verify a recovery registration dependency; retain its current trust and inspect the recovery evidence".into())
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
	registration_sources_with_planned_in(tx, deployment, tenant, node, &[]).await
}

/// Planned sources come only from the same in-TX admission projection used by
/// Rows capture; validate them before any physical unlink.
pub(crate) async fn registration_sources_with_planned_in(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	tenant: &str,
	node: &str,
	planned: &[String],
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
	let receipts = super::peer_admission::terminal::sources_in(tx, tenant, node).await?;
	for source in receipts.iter().chain(planned) {
		let source = NodeAddress::parse(source).map_err(|_| blocked())?;
		if source == owner {
			return Err(blocked());
		}
		if &source.deployment == deployment {
			let holder = super::node_registrations::name_tenant(tx, source.node.as_str())
				.await?
				.ok_or_else(blocked)?;
			// Same-Tenant sibling routes require permanent identity, no registration.
			if holder == tenant {
				continue;
			}
		}
		sources.insert(source.to_string());
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
		let address = NodeAddress::parse(source).map_err(|_| blocked())?;
		let registration = super::node_registrations::get_locked(tx, tenant, node, &address, true)
			.await?
			.ok_or_else(blocked)?;
		if &address.deployment == deployment {
			if !matches!(
				&registration.counterpart,
				aidash_domain::nodes::registration::Counterpart::SameDeployment
			) {
				return Err(blocked());
			}
		} else {
			let owner =
				aidash_application::ports::federation::inbound::ReceivingKeyOwner::Registration {
					tenant: aidash_domain::tenancy::TenantName::new(tenant)?,
					node: NodeName::parse(node)?,
					source_node: address,
				};
			let keys = super::receiving_keys::get(tx, &owner)
				.await?
				.ok_or_else(blocked)?;
			keys.keys.current.validate().map_err(|_| blocked())?;
			if let Some(retiring) = &keys.keys.retiring {
				retiring.key.validate().map_err(|_| blocked())?;
				retiring.schedule.validate().map_err(|_| blocked())?;
			}
			let aidash_domain::nodes::registration::Counterpart::OtherDeployment { key, .. } =
				registration.counterpart
			else {
				return Err(blocked());
			};
			if aidash_application::federation::registrations::trusted_keys(&key)?.current
				!= keys.keys.current
			{
				return Err(blocked());
			}
		}
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
