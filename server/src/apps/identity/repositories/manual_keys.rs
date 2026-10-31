//! Crypto-only administration of an existing receiving record. The service
//! retains current policy and catalog ownership locks before entering here.
use super::super::serializers::manual_keys::{KeyChange, ReceivingKeyStatus};
use crate::{Error, Result};
use aidash_application::ports::federation::inbound::ReceivingKeyOwner;
use aidash_domain::nodes::keys::TrustedDeploymentKeys;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

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

pub(crate) async fn control(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
	change: Option<&KeyChange>,
) -> Result<ReceivingKeyStatus> {
	let (table, condition) = selector(owner);
	let mut query = Query::select();
	query
		.columns(["source_keys", "keys_revision", "revision", "enabled"].map(Alias::new))
		.from(Alias::new(table))
		.cond_where(condition.clone());
	if change.is_some() {
		query.lock(LockType::Update);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	let row = tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or_else(|| Error::NotFound("receiving record not found".into()))?;
	let row = QueryRow::from_backend_row(row).data;
	let encoded: Option<String> = serde_json::from_value(row["source_keys"].clone())?;
	let mut keys: TrustedDeploymentKeys = serde_json::from_str(&encoded.ok_or_else(|| {
		Error::Unprocessable("this record has no foreign Deployment Key".into())
	})?)?;
	keys.current.validate()?;
	if let Some(retiring) = &keys.retiring {
		retiring.key.validate()?;
		retiring.schedule.validate()?;
	}
	let mut keys_revision: i64 = serde_json::from_value(row["keys_revision"].clone())?;
	let authorization_revision: i64 = serde_json::from_value(row["revision"].clone())?;
	let enabled: bool = serde_json::from_value(row["enabled"].clone())?;
	if let Some(change) = change {
		let replacement = change.replacement()?;
		if change.expected_keys_revision != keys_revision
			|| change.expected_authorization_revision != authorization_revision
		{
			return Err(Error::Conflict("receiving record revision changed".into()));
		}
		if let Some(replacement) = replacement {
			keys.manual_repin(replacement)?;
		} else {
			keys.quarantine();
		}
		let pem = aidash_capability::public_key_pem(&keys.current.public_key)
			.map_err(|_| Error::Invalid("invalid receiving key".into()))?;
		let (sql, values) = Query::update()
			.table(Alias::new(table))
			.value(
				Alias::new("source_keys"),
				Expr::value(serde_json::to_string(&keys)?),
			)
			.value(Alias::new("keys_revision"), Expr::value(keys_revision + 1))
			.value(
				Alias::new("source_key_id"),
				Expr::value(keys.current.key_id.to_string()),
			)
			.value(Alias::new("source_public_key_pem"), Expr::value(pem))
			.cond_where(condition)
			.and_where(Expr::col("keys_revision").eq(Expr::value(keys_revision)))
			.and_where(Expr::col("revision").eq(Expr::value(authorization_revision)))
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			!= 1
		{
			return Err(Error::Conflict("receiving record revision changed".into()));
		}
		keys_revision += 1;
	}
	Ok(ReceivingKeyStatus {
		key_id: keys.current.key_id.to_string(),
		quarantined: keys.quarantined,
		keys_revision,
		authorization_revision,
		enabled,
	})
}
