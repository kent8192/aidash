//! A bank pins its Registry policy; callers cannot substitute larger read/storage caps.
use super::{access::Lease, native_memory::bank_id};
use crate::{Error, Result, database::native};
use aidash_domain::{memory::Bank, registry::EntityRef};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, Func, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Settings {
	pub provider: EntityRef,
	pub revision: i64,
}
pub(crate) async fn get(lease: &mut Lease<'_>, bank: &Bank) -> Result<Option<Settings>> {
	let Some(id) = bank_id(lease, bank, false).await? else {
		return Ok(None);
	};
	native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_bank_settings"))
			.and_where(Expr::col("bank_id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.map(|row| -> Result<Settings> {
		Ok(Settings {
			provider: EntityRef {
				id: row.try_get("provider_id")?,
				version: row.try_get("provider_version")?,
			},
			revision: row.try_get("revision")?,
		})
	})
	.transpose()
}
pub(crate) async fn ensure(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
) -> Result<Settings> {
	if let Some(settings) = get(lease, bank).await? {
		if settings.provider != *provider {
			return Err(Error::Conflict(
				"memory bank policy changed; select its exact Registry version".into(),
			));
		}
		return Ok(settings);
	}
	if bank.participant.is_none() {
		super::units::authorize(lease, bank, "memory.configure").await?;
	}
	set(lease, bank, provider, 0).await
}
pub(crate) async fn set(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
	expected: i64,
) -> Result<Settings> {
	let id = bank_id(lease, bank, true).await?.ok_or(Error::Forbidden)?;
	let current = get(lease, bank).await?;
	if expected < 0 || current.as_ref().map_or(0, |c| c.revision) != expected {
		return Err(Error::Conflict(
			"observed memory bank policy revision changed".into(),
		));
	}
	// Workspace mutation locks also cover policy replacement. Tombstones count
	// toward record capacity; every non-deleted row counts toward live capacity.
	// Mutation receipts and model operations remain durable after body retention.
	// Rejection rolls back both settings and any participant update in this lease.
	let policy = crate::semantic::native_memory::policy(lease, provider).await?;
	for (table, live_only, cap) in [
		("memory_units", false, policy.retention.max_unit_records),
		("memory_units", true, policy.bounds.max_units),
		(
			"memory_receipts",
			false,
			policy.retention.max_model_operations,
		),
		(
			"memory_model_operations",
			false,
			policy.retention.max_model_operations,
		),
	] {
		let mut query = Query::select();
		query
			.expr(Func::count(Expr::col(ColumnRef::Asterisk).into()))
			.from(Alias::new(table))
			.and_where(Expr::col("bank_id").eq(Expr::value(id)));
		if live_only {
			query.and_where(Expr::col("deleted").eq(false));
		}
		let count: i64 = native::query_scalar(&query.to_string(PostgresQueryBuilder))
			.scalar_one(&mut **lease.tx())
			.await?;
		if usize::try_from(count).ok().is_none_or(|count| count > cap) {
			return Err(Error::Conflict(
				"replacement memory policy is below existing bank storage".into(),
			));
		}
	}
	if let Some(current) = current.as_ref() {
		let previous = crate::semantic::native_memory::policy(lease, &current.provider).await?;
		if policy.bounds.max_graph_visits < previous.bounds.max_graph_visits {
			let rows = native::query(
				&Query::select()
					.column(Alias::new("evidence"))
					.from(Alias::new("memory_units"))
					.and_where(Expr::col("bank_id").eq(Expr::value(id)))
					.and_where(Expr::col("deleted").eq(false))
					.and_where(Expr::col("stale").eq(false))
					.limit(policy.bounds.max_units as u64 + 1)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **lease.tx())
			.await?;
			for row in rows {
				let evidence: Vec<aidash_domain::memory::Evidence> = row.try_get("evidence")?;
				match super::units::current(
					lease,
					bank.workspace,
					&evidence,
					policy.bounds.max_graph_visits,
				)
				.await
				{
					Ok(()) | Err(Error::Forbidden | Error::Conflict(_)) => {}
					Err(Error::Invalid(_)) => {
						return Err(Error::Conflict(
							"replacement memory policy is below existing provenance".into(),
						));
					}
					Err(error) => return Err(error),
				}
			}
		}
	}
	let revision = expected
		.checked_add(1)
		.filter(|v| *v < i64::MAX)
		.ok_or_else(|| Error::Invalid("memory bank policy revision exhausted".into()))?;
	let values = [
		"bank_id",
		"provider_id",
		"provider_version",
		"revision",
		"next_maintenance",
	]
	.map(Alias::new);
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_bank_settings"))
			.columns(values)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(&provider.id))
					.expr(Expr::value(&provider.version))
					.expr(Expr::value(revision))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.on_conflict(
				reinhardt::query::OnConflict::column(Alias::new("bank_id"))
					.update_columns(
						[
							"provider_id",
							"provider_version",
							"revision",
							"next_maintenance",
						]
						.map(Alias::new),
					)
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(Settings {
		provider: provider.clone(),
		revision,
	})
}

/// A network retry uses the same request identity; an obsolete receipt never changes policy again.
pub(crate) async fn configure(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
	expected: i64,
	operation: uuid::Uuid,
	capacity: usize,
) -> Result<Settings> {
	if operation.is_nil() {
		return Err(Error::Invalid(
			"policy change operation ID is required".into(),
		));
	}
	let id = bank_id(lease, bank, true).await?.ok_or(Error::Forbidden)?;
	let digest = aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(&(
		bank, provider, expected,
	))?);
	if let Some(receipt) = native::query(
		&Query::select()
			.columns(["bank_id", "digest", "applied_revision"].map(Alias::new))
			.from(Alias::new("memory_bank_policy_changes"))
			.and_where(Expr::col("operation_id").eq(Expr::value(operation)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	{
		if receipt.try_get::<uuid::Uuid>("bank_id")? != id
			|| receipt.try_get::<String>("digest")? != digest
		{
			return Err(Error::Conflict(
				"policy change key reused for a different request".into(),
			));
		}
		let current = get(lease, bank).await?.ok_or(Error::Forbidden)?;
		if current.provider != *provider
			|| current.revision != receipt.try_get::<i64>("applied_revision")?
		{
			return Err(Error::Conflict(
				"completed policy change has since been superseded".into(),
			));
		}
		return Ok(current);
	}
	super::native_memory::record_capacity(lease, id, "memory_bank_policy_changes", capacity)
		.await?;
	let settings = set(lease, bank, provider, expected).await?;
	let actor = lease.saved()?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_bank_policy_changes"))
			.columns(
				[
					"operation_id",
					"bank_id",
					"digest",
					"applied_revision",
					"actor",
					"created_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(operation))
					.expr(Expr::value(id))
					.expr(Expr::value(digest))
					.expr(Expr::value(settings.revision))
					.expr(Expr::value(actor))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(settings)
}
