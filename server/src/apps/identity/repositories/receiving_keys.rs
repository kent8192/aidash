//! Authentication-only receiving trust. Every operation runs in its owner's
//! Tenant transaction; callers retain the current policy and Node locks.
use super::super::models::authority_control;
use crate::{Error, Result};
use aidash_application::ports::federation::inbound::{ReceivingKeyOwner, ReceivingKeys};
use aidash_domain::nodes::keys::TrustedDeploymentKeys;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub(crate) fn tenant(owner: &ReceivingKeyOwner) -> &aidash_domain::tenancy::TenantName {
	match owner {
		ReceivingKeyOwner::Registration { tenant, .. } => tenant,
	}
}

fn selector(owner: &ReceivingKeyOwner) -> (&'static str, Condition) {
	match owner {
		ReceivingKeyOwner::Registration {
			tenant,
			node,
			source_node,
		} => (
			"node_registrations",
			Condition::all()
				.add(Expr::col("tenant").eq(Expr::value(tenant.as_str())))
				.add(Expr::col("node").eq(Expr::value(node.as_str())))
				.add(Expr::col("source_node").eq(Expr::value(source_node.to_string()))),
		),
	}
}

async fn read(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
	lock: bool,
) -> Result<Option<(ReceivingKeys, i64, bool)>> {
	let (table, condition) = selector(owner);
	let mut query = Query::select();
	query
		.columns(["source_keys", "keys_revision", "revision", "enabled"].map(Alias::new))
		.from(Alias::new(table))
		.cond_where(condition);
	if lock {
		query.lock(LockType::Update);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? else {
		return Ok(None);
	};
	let failed = reinhardt::core::exception::Error::from;
	let encoded: String = row.get("source_keys").map_err(failed)?;
	Ok(Some((
		ReceivingKeys {
			keys: serde_json::from_str(&encoded)?,
			keys_revision: row.get("keys_revision").map_err(failed)?,
		},
		row.get("revision").map_err(failed)?,
		row.get("enabled").map_err(failed)?,
	)))
}

pub(crate) async fn get(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
) -> Result<Option<ReceivingKeys>> {
	Ok(read(tx, owner, false).await?.map(|(keys, _, _)| keys))
}

pub(crate) async fn get_locked(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
) -> Result<Option<ReceivingKeys>> {
	Ok(read(tx, owner, true).await?.map(|(keys, _, _)| keys))
}

pub(crate) async fn advance(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
	expected_keys_revision: i64,
	expected_authorization_revision: i64,
	keys: &TrustedDeploymentKeys,
) -> Result<bool> {
	let Some(snapshot) = read(tx, owner, true).await? else {
		return Ok(false);
	};
	if !snapshot.2 {
		return Ok(false);
	}
	compare_and_advance(
		tx,
		owner,
		expected_keys_revision,
		expected_authorization_revision,
		keys,
		snapshot,
	)
	.await
}

/// The Federation adapter has locked this row and revalidated the exact real
/// durable request in this transaction. This does not grant live authority.
pub(crate) async fn advance_recovery(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
	expected_keys_revision: i64,
	expected_authorization_revision: i64,
	keys: &TrustedDeploymentKeys,
) -> Result<bool> {
	if !matches!(owner, ReceivingKeyOwner::Registration { .. }) {
		return Ok(false);
	}
	let Some(snapshot) = read(tx, owner, true).await? else {
		return Ok(false);
	};
	compare_and_advance(
		tx,
		owner,
		expected_keys_revision,
		expected_authorization_revision,
		keys,
		snapshot,
	)
	.await
}

async fn compare_and_advance(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
	expected_keys_revision: i64,
	expected_authorization_revision: i64,
	keys: &TrustedDeploymentKeys,
	snapshot: (ReceivingKeys, i64, bool),
) -> Result<bool> {
	let (stored, revision, enabled_at_read) = snapshot;
	if revision != expected_authorization_revision
		|| stored.keys_revision != expected_keys_revision
		|| stored.keys.quarantined
		|| expected_keys_revision == i64::MAX
	{
		return Ok(false);
	}

	// A normal transition cannot reset quarantine, retirement history or trust.
	let Some(retiring) = &keys.retiring else {
		return Ok(false);
	};
	let mut expected = stored.keys.clone();
	if expected
		.accept_successor(
			&stored.keys.current.key_id,
			keys.current.clone(),
			retiring.schedule.clone(),
			chrono::Utc::now(),
		)
		.is_err()
		|| expected != *keys
	{
		return Ok(false);
	}
	let pem = aidash_capability::public_key_pem(&keys.current.public_key)
		.map_err(|_| Error::Invalid("invalid receiving key".into()))?;
	let (table, condition) = selector(owner);
	let (sql, values) = Query::update()
		.table(Alias::new(table))
		.value(
			Alias::new("source_keys"),
			Expr::value(serde_json::to_string(keys)?),
		)
		.value(
			Alias::new("keys_revision"),
			Expr::value(expected_keys_revision + 1),
		)
		.value(
			Alias::new("source_key_id"),
			Expr::value(keys.current.key_id.to_string()),
		)
		.value(Alias::new("source_public_key_pem"), Expr::value(pem))
		.cond_where(condition)
		.and_where(Expr::col("keys_revision").eq(Expr::value(expected_keys_revision)))
		.and_where(Expr::col("revision").eq(Expr::value(expected_authorization_revision)))
		.and_where(Expr::col("enabled").eq(Expr::value(enabled_at_read)))
		.build(PostgresQueryBuilder);
	// Classification for this bounded authentication update, not authorization:
	// the same transaction already holds current policy/owner locks and, for
	// recovery, the exact durable proof. Ordinary work retains its atomic gate.
	authority_control(tx).await?;
	Ok(tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		== 1)
}
