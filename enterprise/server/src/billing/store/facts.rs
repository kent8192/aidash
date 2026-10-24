//! Intake and queries of immutable precise facts. The permanent history floor
//! serializes retention before any Tenant or account locks can be acquired.
use super::super::facts::{self, AccountSample, AccountSegment, AccountUsage, History};
use super::super::models::{
	BillingCoveragePeriod, BillingPodUsage, BillingRetentionState, BillingStorageSample,
	BillingUsageSegment,
};
use super::{database_now, deletions, execute, fetch, select};
use aidash_application::{
	tenancy::TenancyScope,
	usage::{PodTime, StorageSample},
};
use aidash_server::{Error, Result, database::native::Pool};
use chrono::{DateTime, Utc};
use reinhardt::{
	db::{backends::TransactionExecutor, orm::Model},
	query::{Alias, Expr, ExprTrait, IntoValue, LockType, OnConflict, Query},
};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(serde::Deserialize)]
struct Floor {
	floor_ns: i64,
}

/// SHARE for intake, UPDATE for sweep; always acquired before lifecycle locks.
/// Initial epoch floor is narrowed by each permanent Tenant's recorded birth.
pub(super) async fn retention(tx: &mut dyn TransactionExecutor, exclusive: bool) -> Result<i64> {
	execute(
		tx,
		Query::insert()
			.into_table(Alias::new(BillingRetentionState::table_name()))
			.columns(["id", "floor_ns"].map(Alias::new))
			.values_panic([
				IntoValue::into_value("history"),
				IntoValue::into_value(0_i64),
			])
			.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned()),
	)
	.await?;
	let query = select(BillingRetentionState::table_name(), &["floor_ns"])
		.and_where(Expr::col("id").eq(Expr::value("history")))
		.lock(if exclusive {
			LockType::Update
		} else {
			LockType::Share
		})
		.to_owned();
	fetch::<Floor>(tx, &query)
		.await?
		.pop()
		.map(|r| r.floor_ns)
		.ok_or_else(|| Error::Invalid("billing retention history boundary unavailable".into()))
}

#[derive(serde::Deserialize)]
struct Birth {
	created_at: DateTime<Utc>,
}
#[derive(serde::Deserialize)]
struct TenantRow {
	tenant: String,
}
#[derive(serde::Deserialize)]
struct Purged {
	purged_at: Option<DateTime<Utc>>,
}

async fn lifecycle(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<(i64, i64)> {
	// This row is Tenant-owned. Intake runs in the named Tenant scope, including
	// a deleted Tenant; no live authorization boundary is invoked by this sink.
	let policy = select("authorization_bundles", &["tenant"])
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.lock(LockType::Share)
		.to_owned();
	let policy = fetch::<TenantRow>(tx, &policy).await?;
	if policy.iter().any(|row| row.tenant != tenant) {
		return Err(Error::Forbidden);
	}
	aidash_server::authorization::services::nodes::lock_capacity(tx, tenant).await?;
	let root = select("tenants", &["created_at"])
		.and_where(Expr::col("name").eq(Expr::value(tenant)))
		.to_owned();
	let birth = fetch::<Birth>(tx, &root)
		.await?
		.pop()
		.ok_or_else(|| Error::NotFound("permanent Tenant history".into()))?;
	let birth = facts::nanos(birth.created_at)?;
	if policy.is_empty() {
		// Purge takes deletion UPDATE before deleting policy. Never request this
		// row while holding policy SHARE: that would invert its real lock order.
		let query = select("tenant_deletions", &["purged_at"])
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("restored_at").is_null())
			.lock(LockType::Share)
			.to_owned();
		if !fetch::<Purged>(tx, &query)
			.await?
			.iter()
			.any(|row| row.purged_at.is_some())
		{
			return Err(Error::Conflict(
				"Tenant lifecycle history unavailable; retry after purge completes".into(),
			));
		}
	}
	let now = facts::nanos(database_now(tx).await?)?;
	Ok((birth, now))
}

async fn history(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	birth: i64,
	now: i64,
) -> Result<History> {
	let query = select(
		BillingCoveragePeriod::table_name(),
		&[
			"id",
			"tenant",
			"account_id",
			"starts_at",
			"ends_at",
			"consented_by_administrator",
			"consented_by_owner",
		],
	)
	.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
	.to_owned();
	let coverage = fetch::<BillingCoveragePeriod>(tx, &query).await?;
	let mut deletion = deletions(tx, &[tenant.to_owned()]).await?;
	History::new(
		coverage,
		&deletion.remove(tenant).unwrap_or_default(),
		birth,
		now,
	)
}

const POD_COLUMNS: &[&str] = &[
	"allocation",
	"receipt_digest",
	"tenant",
	"started_at",
	"ended_at",
	"started_ns",
	"ended_ns",
	"billable_ns",
	"millis",
];
async fn allocation(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<Option<BillingPodUsage>> {
	let query = select(BillingPodUsage::table_name(), POD_COLUMNS)
		.and_where(Expr::col("allocation").eq(Expr::value(id)))
		.to_owned();
	Ok(fetch::<BillingPodUsage>(tx, &query).await?.pop())
}
fn duplicate(existing: &BillingPodUsage, event: &PodTime, start: i64, end: i64) -> Result<()> {
	if existing.tenant != event.tenant
		|| existing.receipt_digest != event.receipt_digest
		|| existing.started_ns != start
		|| existing.ended_ns != end
	{
		return Err(Error::Conflict(
			"allocation receipt conflicts with immutable Tenant, digest or lifetime".into(),
		));
	}
	Ok(())
}
fn known(start: i64, birth: i64, floor: i64) -> Result<()> {
	if start < birth.max(floor) {
		return Err(Error::Conflict("usage predates known retained Tenant history; restore evidence before retrying the complete receipt".into()));
	}
	Ok(())
}

pub(crate) async fn pod_time(pool: &Pool, event: PodTime) -> Result<()> {
	let (start, end) = facts::span(event.started_at, event.ended_at)?;
	if event.allocation.is_nil()
		|| event.receipt_digest.len() != 64
		|| !event
			.receipt_digest
			.bytes()
			.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	{
		return Err(Error::Invalid(
			"allocation requires a nonnil identity and canonical lowercase SHA256 receipt digest"
				.into(),
		));
	}
	let scope = TenancyScope::tenant(&event.tenant)?;
	pool.atomic(&scope, async |tx| {
		let floor = retention(tx, false).await?;
		if let Some(existing) = allocation(tx, event.allocation).await? {
			return duplicate(&existing, &event, start, end);
		}
		let (birth, now) = lifecycle(tx, &event.tenant).await?;
		if end > now {
			return Err(Error::Invalid(
				"allocation receipt ends in the future".into(),
			));
		}
		known(start, birth, floor)?;
		let pieces = history(tx, &event.tenant, birth, now)
			.await?
			.partition(start, end)?;
		let billable: i64 = pieces
			.iter()
			.filter(|p| p.status != "exempt")
			.map(|p| p.ends_ns - p.starts_ns)
			.sum();
		// Another Tenant can concurrently submit the same allocation. The unique
		// key arbitrates before any segment is written; compare the winner's
		// entire immutable payload instead of acknowledging a conflicting report.
		let query = Query::insert()
			.into_table(Alias::new(BillingPodUsage::table_name()))
			.columns(POD_COLUMNS.iter().copied().map(Alias::new))
			.values_panic([
				IntoValue::into_value(event.allocation),
				IntoValue::into_value(event.receipt_digest.clone()),
				IntoValue::into_value(event.tenant.clone()),
				IntoValue::into_value(facts::legacy_timestamp(start)),
				IntoValue::into_value(facts::legacy_timestamp(end)),
				IntoValue::into_value(start),
				IntoValue::into_value(end),
				IntoValue::into_value(billable),
				IntoValue::into_value(billable / 1_000_000),
			])
			.on_conflict(
				OnConflict::column(Alias::new("allocation"))
					.do_nothing()
					.to_owned(),
			)
			.returning([Alias::new("allocation")])
			.to_owned();
		let (sql, values) = reinhardt::query::QueryStatementBuilder::build(
			&query,
			reinhardt::query::PostgresQueryBuilder,
		);
		let inserted = tx
			.fetch_optional(&sql, reinhardt::db::orm::execution::convert_values(values))
			.await?
			.is_some();
		if !inserted {
			let existing = allocation(tx, event.allocation)
				.await?
				.ok_or_else(|| Error::Conflict("allocation receipt identity unavailable".into()))?;
			return duplicate(&existing, &event, start, end);
		}
		for piece in pieces {
			execute(
				tx,
				Query::insert()
					.into_table(Alias::new(BillingUsageSegment::table_name()))
					.columns(
						[
							"id",
							"allocation",
							"tenant",
							"starts_ns",
							"ends_ns",
							"status",
							"coverage_id",
							"account_id",
						]
						.map(Alias::new),
					)
					.values_panic([
						IntoValue::into_value(Uuid::new_v4()),
						IntoValue::into_value(event.allocation),
						IntoValue::into_value(event.tenant.clone()),
						IntoValue::into_value(piece.starts_ns),
						IntoValue::into_value(piece.ends_ns),
						IntoValue::into_value(piece.status),
						IntoValue::into_value(piece.coverage_id),
						IntoValue::into_value(piece.account_id),
					]),
			)
			.await?;
		}
		Ok(())
	})
	.await
}

pub(crate) async fn storage_sampled(pool: &Pool, samples: Vec<StorageSample>) -> Result<()> {
	let mut tenants = BTreeMap::<String, Vec<StorageSample>>::new();
	for sample in samples {
		tenants
			.entry(sample.tenant.clone())
			.or_default()
			.push(sample);
	}
	// A Tenant's daily facts commit atomically. A partially completed multi-
	// Tenant pass safely retries through the existing once-per-day identity.
	for (tenant, samples) in tenants {
		let scope = TenancyScope::tenant(&tenant)?;
		pool.atomic(&scope, async |tx| {
			let floor = retention(tx, false).await?;
			let (birth, now) = lifecycle(tx, &tenant).await?;
			for sample in samples {
				let at = facts::nanos(sample.sampled_at)?;
				let bytes = i64::try_from(sample.bytes).map_err(|_| {
					Error::Invalid("storage bytes exceed the exact supported integer range".into())
				})?;
				if at > now {
					return Err(Error::Invalid("storage sample is in the future".into()));
				}
				let query = select(BillingStorageSample::table_name(), &["tenant"])
					.and_where(Expr::col("tenant").eq(Expr::value(tenant.clone())))
					.and_where(
						Expr::col("sampled_on").eq(super::date(sample.sampled_at.date_naive())),
					)
					.to_owned();
				if !fetch::<TenantRow>(tx, &query).await?.is_empty() {
					continue;
				}
				known(at, birth, floor)?;
				let point = history(tx, &tenant, birth, now).await?.point(at);
				execute(
					tx,
					Query::insert()
						.into_table(Alias::new(BillingStorageSample::table_name()))
						.columns(
							[
								"id",
								"tenant",
								"sampled_on",
								"sampled_at",
								"bytes",
								"sampled_ns",
								"status",
								"coverage_id",
								"account_id",
							]
							.map(Alias::new),
						)
						.from_subquery(
							Query::select()
								.expr(Expr::value(Uuid::new_v4()))
								.expr(Expr::value(tenant.clone()))
								.expr(super::date(sample.sampled_at.date_naive()))
								.expr(Expr::value(facts::legacy_timestamp(at)))
								.expr(Expr::value(bytes))
								.expr(Expr::value(at))
								.expr(Expr::value(point.status))
								.expr(Expr::value(point.coverage_id))
								.expr(Expr::value(point.account_id))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns(["tenant", "sampled_on"].map(Alias::new))
								.do_nothing()
								.to_owned(),
						),
				)
				.await?;
			}
			Ok(())
		})
		.await?;
	}
	Ok(())
}

pub async fn account_usage(
	pool: &Pool,
	account: Uuid,
	start: DateTime<Utc>,
	end: DateTime<Utc>,
) -> Result<AccountUsage> {
	let (from, until) = facts::span(start, end)?;
	let segments = select(
		BillingUsageSegment::table_name(),
		&[
			"id",
			"allocation",
			"tenant",
			"starts_ns",
			"ends_ns",
			"status",
			"coverage_id",
			"account_id",
		],
	)
	.and_where(Expr::col("account_id").eq(Expr::value(account)))
	.and_where(Expr::col("starts_ns").lt(Expr::value(until)))
	.and_where(Expr::col("ends_ns").gt(Expr::value(from)))
	.order_by(Alias::new("starts_ns"), reinhardt::query::Order::Asc)
	.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
	.to_owned();
	let samples = select(
		BillingStorageSample::table_name(),
		&[
			"id",
			"tenant",
			"sampled_on",
			"sampled_at",
			"bytes",
			"sampled_ns",
			"status",
			"coverage_id",
			"account_id",
		],
	)
	.and_where(Expr::col("account_id").eq(Expr::value(account)))
	.and_where(Expr::col("sampled_ns").gte(Expr::value(from)))
	.and_where(Expr::col("sampled_ns").lt(Expr::value(until)))
	.order_by(Alias::new("sampled_ns"), reinhardt::query::Order::Asc)
	.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
	.to_owned();
	let (segments, samples) = pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			// Keep parents and coverage alive throughout both reads and clipping.
			retention(tx, false).await?;
			Ok((
				fetch::<BillingUsageSegment>(tx, &segments).await?,
				fetch::<BillingStorageSample>(tx, &samples).await?,
			))
		})
		.await?;
	let mut total = 0_u128;
	let mut clipped = Vec::new();
	for segment in segments {
		let (a, z) = (segment.starts_ns.max(from), segment.ends_ns.min(until));
		total +=
			u128::try_from(z - a).map_err(|_| Error::Invalid("invalid frozen segment".into()))?;
		clipped.push(AccountSegment {
			allocation: segment.allocation,
			tenant: segment.tenant,
			coverage_id: segment
				.coverage_id
				.ok_or_else(|| Error::Invalid("covered usage lacks provenance".into()))?,
			starts_at: DateTime::from_timestamp_nanos(a),
			ends_at: DateTime::from_timestamp_nanos(z),
			nanos: (z - a).to_string(),
		});
	}
	let samples = samples
		.into_iter()
		.map(|s| {
			Ok(AccountSample {
				id: s.id,
				tenant: s.tenant,
				coverage_id: s
					.coverage_id
					.ok_or_else(|| Error::Invalid("covered sample lacks provenance".into()))?,
				sampled_at: DateTime::from_timestamp_nanos(s.sampled_ns),
				bytes: u64::try_from(s.bytes)
					.map_err(|_| Error::Invalid("negative stored bytes".into()))?,
			})
		})
		.collect::<Result<Vec<_>>>()?;
	Ok(AccountUsage {
		account,
		starts_at: start,
		ends_at: end,
		pod_nanos: total.to_string(),
		segments: clipped,
		samples,
	})
}
