//! Ranking-only state: no Unit revision, digest, bank revision, or dependent Run changes.
use super::{access::Lease, bank_settings, native_memory as repository, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{memory::*, registry::EntityRef};
use chrono::{DateTime, Duration, SubsecRound, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) async fn configure(
	lease: &mut Lease<'_>,
	bank: Uuid,
	provider: &EntityRef,
	policy: &Policy,
) -> Result<()> {
	let old = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_bank_decay"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let previous: Option<DateTime<Utc>> = old
		.as_ref()
		.map(|row| row.try_get("activated_at"))
		.transpose()?
		.flatten();
	let next_job = if policy.decay.as_ref().is_some_and(|d| d.dormancy.is_some()) {
		Utc::now()
	} else {
		Utc::now() + Duration::hours(8760)
	};
	let activated = if policy.decay.is_some() {
		Some(previous.unwrap_or_else(Utc::now))
	} else {
		None
	};
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_bank_decay"))
			.columns(
				[
					"bank_id",
					"policy",
					"activated_at",
					"as_of",
					"cursor",
					"next_job",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(bank))
					.expr(Expr::value(serde_json::to_value(provider)?))
					.expr(Expr::value(activated))
					.expr(Expr::value(None::<DateTime<Utc>>))
					.expr(Expr::value(None::<Uuid>))
					.expr(Expr::value(next_job))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("bank_id"))
					.update_columns(
						["policy", "activated_at", "as_of", "cursor", "next_job"].map(Alias::new),
					)
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}

async fn ensure(lease: &mut Lease<'_>, bank: Uuid, id: Uuid) -> Result<()> {
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_unit_retention"))
			.columns(
				[
					"unit_id",
					"bank_id",
					"deliveries",
					"last_delivered_at",
					"reactivated_at",
					"pinned",
					"dormant_policy",
					"changed_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(bank))
					.expr(Expr::value(0_i64))
					.expr(Expr::value(None::<DateTime<Utc>>))
					.expr(Expr::value(None::<DateTime<Utc>>))
					.expr(Expr::value(false))
					.expr(Expr::value(None::<serde_json::Value>))
					.expr(Expr::value(DateTime::<Utc>::UNIX_EPOCH))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("unit_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}

/// A completed local Delivery reactivates; only its first Run journal insertion adds Usage.
pub(crate) async fn delivery(lease: &mut Lease<'_>, unit: &Unit, count: bool) -> Result<()> {
	let bank = repository::bank_id(lease, &unit.bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	ensure(lease, bank, unit.id).await?;
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_unit_retention"))
			.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	if row.try_get::<Uuid>("bank_id")? != bank {
		return Err(Error::Forbidden);
	}
	let mut query = Query::update();
	query
		.table(Alias::new("memory_unit_retention"))
		.value(Alias::new("dormant_policy"), None::<serde_json::Value>)
		.value(Alias::new("reactivated_at"), Utc::now())
		.value(Alias::new("changed_at"), Utc::now())
		.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)));
	if count {
		let deliveries = row
			.try_get::<i64>("deliveries")?
			.checked_add(1)
			.filter(|n| *n < i64::MAX)
			.ok_or_else(|| Error::Conflict("memory Usage exhausted".into()))?;
		query
			.value(Alias::new("deliveries"), deliveries)
			.value(Alias::new("last_delivered_at"), Utc::now());
	}
	native::query(&query.to_string(PostgresQueryBuilder))
		.execute(&mut **lease.tx())
		.await?;
	Ok(())
}

pub(crate) async fn reactivate(lease: &mut Lease<'_>, id: Uuid) -> Result<()> {
	let bank: Uuid = native::query_scalar(
		&Query::select()
			.column(Alias::new("bank_id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	// A never-delivered Unit may have no aggregate yet. Record the new anchor
	// without inventing a Delivery or backfilling Usage from its journal.
	ensure(lease, bank, id).await?;
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(Alias::new("dormant_policy"), None::<serde_json::Value>)
			.value(Alias::new("reactivated_at"), Utc::now())
			.value(Alias::new("changed_at"), Utc::now())
			.and_where(Expr::col("unit_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}

pub(crate) async fn control(
	lease: &mut Lease<'_>,
	bank: &Bank,
	id: Uuid,
	pinned: Option<bool>,
	operation: Uuid,
	digest: &str,
) -> Result<Unit> {
	super::candidates::human(lease)?;
	if operation.is_nil() {
		return Err(Error::Invalid(
			"memory control operation ID is required".into(),
		));
	}
	if let Some(access) = lease.access() {
		let participant: Option<String> = if let Some(id) = bank.participant {
			native::query_scalar(
				&Query::select()
					.column(Alias::new("principal"))
					.from(Alias::new("memory_participants"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **access.tx)
			.await?
		} else {
			None
		};
		let workspace = access.workspace(bank.workspace).await?;
		if participant.as_deref() != Some(access.identity.subject.as_str())
			&& workspace.attributes["owner"].as_str() != Some(access.identity.subject.as_str())
		{
			// Workspace administration uses the existing workspace.update authority.
			access.require(&workspace, "workspace.update").await?;
		}
	}
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let receipt = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_receipts"))
			.and_where(Expr::col("operation_id").eq(Expr::value(operation)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	if let Some(receipt) = &receipt
		&& (receipt.try_get::<Uuid>("bank_id")? != bank_id
			|| receipt.try_get::<String>("digest")? != digest)
	{
		return Err(Error::Conflict(
			"memory operation ID was reused with a different request".into(),
		));
	}
	let unit = units::load(lease, id, false)
		.await?
		.ok_or(Error::Forbidden)?;
	if unit.bank != *bank || !unit.visible() {
		return Err(Error::Forbidden);
	}
	let policy = units::unexpired(lease, &unit).await?;
	units::current(
		lease,
		bank.workspace,
		&unit.content.evidence,
		policy.bounds.max_graph_visits,
	)
	.await?;
	if let Some(receipt) = receipt {
		let outcome: Vec<Evidence> = receipt.try_get("outcome")?;
		if outcome != vec![unit.evidence()] {
			return Err(Error::Conflict(
				"memory operation completed; its result has since changed".into(),
			));
		}
		return Ok(unit);
	}
	repository::record_capacity(
		lease,
		bank_id,
		"memory_receipts",
		policy.retention.max_model_operations,
	)
	.await?;
	// Reserve the exact request before changing the ranking state. Both writes
	// share this authority transaction, so a failed control leaves no receipt.
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_receipts"))
			.columns(["operation_id", "bank_id", "digest", "outcome", "created_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(operation))
					.expr(Expr::value(bank_id))
					.expr(Expr::value(digest))
					.expr(Expr::value(serde_json::to_value(vec![unit.evidence()])?))
					.expr(Expr::value(Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	ensure(lease, bank_id, id).await?;
	if let Some(value) = pinned {
		native::query(
			&Query::update()
				.table(Alias::new("memory_unit_retention"))
				.value(Alias::new("pinned"), value)
				.value(Alias::new("changed_at"), Utc::now())
				.and_where(Expr::col("unit_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	reactivate(lease, id).await?;
	Ok(unit)
}

/// Policy-version mismatch makes a stored Dormant flag inert without deleting it.
pub(crate) fn dormant_filter(
	bank: Uuid,
	provider: &EntityRef,
) -> Result<reinhardt::query::SelectStatement> {
	Ok(Query::select()
		.column(Alias::new("unit_id"))
		.from(Alias::new("memory_unit_retention"))
		.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
		.and_where(Expr::col("dormant_policy").eq(Expr::value(serde_json::to_value(provider)?)))
		.and_where(Expr::col("pinned").eq(false))
		.to_owned())
}

async fn live_support(lease: &mut Lease<'_>, unit: &Unit) -> Result<bool> {
	let settings = bank_settings::get(lease, &unit.bank)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
	let bank = repository::bank_id(lease, &unit.bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	// JSON containment is a typed PostgreSQL expression, not raw SQL.
	// Canonical Unit evidence retains the existing same-Bank admission boundary.
	let evidence = serde_json::to_value(vec![unit.evidence()])?;
	let ids: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
			.and_where(Expr::col("kind").is_in(["observation", "mental_model"]))
			.and_where(Expr::col("deleted").eq(false))
			.and_where(Expr::col("stale").eq(false))
			.and_where(Expr::col("verification").ne("contradicted"))
			.and_where(reinhardt::query::SimpleExpr::CustomWithExpr(
				"? @> ?".into(),
				vec![Expr::col("evidence").into(), Expr::value(evidence).into()],
			))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(policy.bounds.max_units as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **lease.tx())
	.await?;
	if ids.len() > policy.bounds.max_units {
		return Err(Error::Conflict(
			"memory live support exceeds its Bank snapshot bound".into(),
		));
	}
	for id in ids {
		let Some(support) = units::load(lease, id, false).await? else {
			continue;
		};
		let policy = match units::unexpired(lease, &support).await {
			Err(Error::Conflict(_) | Error::Forbidden) => continue,
			result => result?,
		};
		match units::current(
			lease,
			support.bank.workspace,
			&support.content.evidence,
			policy.bounds.max_graph_visits,
		)
		.await
		{
			Ok(()) => return Ok(true),
			Err(Error::Conflict(_) | Error::Forbidden) => continue,
			Err(error) => return Err(error),
		}
	}
	Ok(false)
}

pub(crate) async fn scores(
	lease: &mut Lease<'_>,
	bank: &Bank,
	policy: &Policy,
	units: &[Unit],
	as_of: DateTime<Utc>,
) -> Result<BTreeMap<Uuid, f64>> {
	let Some(decay) = &policy.decay else {
		return Ok(BTreeMap::new());
	};
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let activation: Option<DateTime<Utc>> = native::query_scalar(
		&Query::select()
			.column(Alias::new("activated_at"))
			.from(Alias::new("memory_bank_decay"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **lease.tx())
	.await?
	.flatten();
	let activated =
		activation.ok_or_else(|| Error::Conflict("decay activation is missing".into()))?;
	let mut result = BTreeMap::new();
	for unit in units {
		let row = native::query(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("memory_unit_retention"))
				.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
				.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?;
		let pinned = row
			.as_ref()
			.map(|r| r.try_get::<bool>("pinned"))
			.transpose()?
			.unwrap_or(false);
		let n = row
			.as_ref()
			.map(|r| r.try_get::<i64>("deliveries"))
			.transpose()?
			.unwrap_or(0) as u64;
		let last = row
			.as_ref()
			.map(|r| r.try_get("last_delivered_at"))
			.transpose()?
			.flatten();
		let reactivated = row
			.as_ref()
			.map(|r| r.try_get("reactivated_at"))
			.transpose()?
			.flatten();
		let score = if decay::exempt(unit, decay, pinned, live_support(lease, unit).await?) {
			1.0
		} else {
			decay::retention_score(
				decay,
				unit.learned_at,
				last,
				n,
				activated,
				reactivated,
				as_of,
			)
		};
		result.insert(unit.id, score);
	}
	Ok(result)
}

/// One persisted job per Bank; each invocation commits one ID-ordered page.
pub(crate) async fn sweep(store: &Store) -> Result<usize> {
	let rows = native::query(
		&Query::select()
			.column(("memory_banks", "id"))
			.from(Alias::new("memory_bank_decay"))
			.join(
				reinhardt::query::JoinType::InnerJoin,
				Alias::new("memory_banks"),
				Expr::col(("memory_bank_decay", "bank_id")).equals(("memory_banks", "id")),
			)
			.and_where(Expr::col("activated_at").is_not_null())
			.and_where(Expr::col("next_job").lte(Expr::value(Utc::now())))
			.and_where(Expr::col("home").eq(store.node_id.as_str()))
			.order_by(Alias::new("next_job"), Order::Asc)
			.order_by(("memory_banks", "id"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await?;
	let mut processed = 0;
	for row in rows {
		let mut lease =
			Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
		let result = page(&mut lease, row.try_get("id")?, Utc::now()).await;
		match lease.finish(result).await {
			Ok(n) => processed += n,
			Err(Error::Forbidden | Error::Conflict(_)) => {}
			Err(e) => return Err(e),
		}
	}
	Ok(processed)
}

pub(crate) async fn page(
	lease: &mut Lease<'_>,
	bank_id: Uuid,
	now: DateTime<Utc>,
) -> Result<usize> {
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col("id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	let bank = Bank {
		home: row.try_get("home")?,
		tenant: row.try_get("tenant")?,
		workspace: row.try_get("workspace_id")?,
		participant: row.try_get("participant_id")?,
	};
	repository::lock_workspace(lease, bank.workspace, false).await?;
	units::authorize(lease, &bank, "memory.read").await?;
	let settings = bank_settings::get(lease, &bank)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
	let Some(decay) = &policy.decay else {
		return Ok(0);
	};
	let Some(dormancy) = &decay.dormancy else {
		native::query(
			&Query::update()
				.table(Alias::new("memory_bank_decay"))
				.value(Alias::new("next_job"), now + Duration::hours(8760))
				.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		return Ok(0);
	};
	let job = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_bank_decay"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.lock(LockType::Update)
			.lock_behavior(reinhardt::query::LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let Some(job) = job else {
		return Ok(0);
	};
	if job.try_get::<DateTime<Utc>>("next_job")? > now {
		return Ok(0);
	}
	if job.try_get::<EntityRef>("policy")? != settings.provider {
		return Err(Error::Conflict("decay job policy changed".into()));
	}
	let activated: DateTime<Utc> = job
		.try_get::<Option<DateTime<Utc>>>("activated_at")?
		.ok_or(Error::Forbidden)?;
	let as_of = job
		.try_get::<Option<DateTime<Utc>>>("as_of")?
		// The first page must use the same microsecond precision that PostgreSQL
		// persists for subsequent pages and restart/replay at a threshold boundary.
		.unwrap_or_else(|| now.trunc_subsecs(6));
	let cursor: Option<Uuid> = job.try_get("cursor")?;
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.from(Alias::new("memory_units"))
		.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
		.order_by(Alias::new("id"), Order::Asc)
		.limit(dormancy.batch.min(policy.bounds.max_units) as u64);
	if let Some(cursor) = cursor {
		query.and_where(Expr::col("id").gt(Expr::value(cursor)));
	}
	let ids: Vec<Uuid> = native::query_scalar(&query.to_string(PostgresQueryBuilder))
		.scalar_all(&mut **lease.tx())
		.await?;
	let cutoff = as_of - Duration::hours(i64::from(dormancy.interval_hours));
	for id in &ids {
		let unit = units::load(lease, *id, false)
			.await?
			.ok_or(Error::Forbidden)?;
		if !unit.visible() || unit.updated_at > as_of {
			continue;
		}
		ensure(lease, bank_id, *id).await?;
		let usage = native::query(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("memory_unit_retention"))
				.and_where(Expr::col("unit_id").eq(Expr::value(*id)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **lease.tx())
		.await?;
		let last: Option<DateTime<Utc>> = usage.try_get("last_delivered_at")?;
		let reactivated: Option<DateTime<Utc>> = usage.try_get("reactivated_at")?;
		if last.is_some_and(|last| last > as_of)
			|| usage.try_get::<DateTime<Utc>>("changed_at")? > as_of
		{
			continue;
		}
		let exempt = decay::exempt(
			&unit,
			decay,
			usage.try_get("pinned")?,
			live_support(lease, &unit).await?,
		);
		let eligible = last
			.unwrap_or(unit.learned_at)
			.max(activated)
			.max(reactivated.unwrap_or(activated))
			<= cutoff && decay::retention_score(
			decay,
			unit.learned_at,
			last,
			usage.try_get::<i64>("deliveries")? as u64,
			activated,
			reactivated,
			cutoff,
		) < f64::from(dormancy.threshold_millionths) / 1_000_000.0;
		native::query(
			&Query::update()
				.table(Alias::new("memory_unit_retention"))
				.value(
					Alias::new("dormant_policy"),
					if !exempt && eligible {
						Some(serde_json::to_value(&settings.provider)?)
					} else {
						None
					},
				)
				.and_where(Expr::col("unit_id").eq(Expr::value(*id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	let done = ids.len() < dormancy.batch.min(policy.bounds.max_units);
	native::query(
		&Query::update()
			.table(Alias::new("memory_bank_decay"))
			.value(Alias::new("as_of"), if done { None } else { Some(as_of) })
			.value(
				Alias::new("cursor"),
				if done { None } else { ids.last().copied() },
			)
			.value(
				Alias::new("next_job"),
				if done {
					now + Duration::hours(i64::from(dormancy.interval_hours))
				} else {
					now
				},
			)
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(ids.len())
}

#[cfg(test)]
#[path = "memory_decay/tests.rs"]
mod tests;
