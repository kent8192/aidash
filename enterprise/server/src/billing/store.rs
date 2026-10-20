//! The billing app's queries. Every table it owns is Deployment-wide, and so
//! are the Community tables it reads (`tenants`, `tenant_deletions`), so these
//! run in the no-Tenant scope. Mappings are Tenant-owned: they are read in
//! their Tenant's own scope.
use super::models::{
	BillingAccount, BillingAttachment, BillingAttachmentConsent, BillingPodUsage,
	BillingStorageSample, BillingSubscription,
};
use super::payments::{Status, Subscription};
use aidash_application::tenancy::TenancyScope;
use aidash_server::database::native::Pool;
use aidash_server::{Error, Result};
use chrono::{DateTime, NaiveDate, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SelectStatement, SimpleExpr,
};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Every row `query` selects, read as `T`.
pub async fn fetch<T: DeserializeOwned>(
	tx: &mut dyn TransactionExecutor,
	query: &SelectStatement,
) -> Result<Vec<T>> {
	let (sql, values) = query.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| {
			Ok(serde_json::from_value(
				QueryRow::from_backend_row(row).data,
			)?)
		})
		.collect()
}

fn select(table: &str, columns: &[&str]) -> SelectStatement {
	let mut query = Query::select();
	query
		.columns(columns.iter().copied().map(Alias::new))
		.from(Alias::new(table));
	query
}

/// A date as the database's own type: it travels as text, which PostgreSQL
/// will not assign to a `date` column on its own.
fn date(day: NaiveDate) -> SimpleExpr {
	Expr::value(day.to_string()).cast_as(Alias::new("date"))
}

pub async fn execute(
	tx: &mut dyn TransactionExecutor,
	statement: &impl QueryStatementBuilder,
) -> Result<()> {
	let (sql, values) = statement.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

// Subscriptions

#[derive(serde::Deserialize)]
struct SubscriptionRow {
	tier: String,
	status: String,
	grace_ends_at: Option<DateTime<Utc>>,
}

pub async fn subscription(pool: &Pool, account: Uuid) -> Result<Option<Subscription>> {
	let query = select(
		BillingSubscription::table_name(),
		&["tier", "status", "grace_ends_at"],
	)
	.and_where(Expr::col("account_id").eq(Expr::value(account)))
	.to_owned();
	let rows: Vec<SubscriptionRow> = pool
		.atomic(&TenancyScope::NoTenant, async |tx| fetch(tx, &query).await)
		.await?;
	rows.into_iter()
		.next()
		.map(|row| {
			Ok(Subscription {
				tier: row.tier,
				status: Status::parse(&row.status)?,
				grace_ends_at: row.grace_ends_at,
			})
		})
		.transpose()
}

/// Stores what the payment processor reported for `account`, replacing what
/// was there. The processor's adapter calls this when it learns of a change.
pub async fn record_subscription(
	pool: &Pool,
	account: Uuid,
	subscription: &Subscription,
) -> Result<()> {
	let statement = Query::insert()
		.into_table(Alias::new(BillingSubscription::table_name()))
		.columns(
			[
				"account_id",
				"tier",
				"status",
				"grace_ends_at",
				"updated_at",
			]
			.map(Alias::new),
		)
		.values_panic([
			IntoValue::into_value(account),
			IntoValue::into_value(subscription.tier.clone()),
			IntoValue::into_value(subscription.status.as_str().to_owned()),
			IntoValue::into_value(subscription.grace_ends_at),
			IntoValue::into_value(Utc::now()),
		])
		.on_conflict(
			OnConflict::column(Alias::new("account_id"))
				.update_columns(["tier", "status", "grace_ends_at", "updated_at"].map(Alias::new))
				.to_owned(),
		)
		.to_owned();
	pool.atomic(&TenancyScope::NoTenant, async |tx| {
		execute(tx, &statement).await?;
		Ok(())
	})
	.await
}

// Accounts and attachments

pub async fn create_account(pool: &Pool, owner: Uuid, label: &str) -> Result<BillingAccount> {
	let account = BillingAccount {
		id: Uuid::new_v4(),
		owner_identity: owner,
		label: label.to_owned(),
		created_at: Utc::now(),
	};
	let statement = Query::insert()
		.into_table(Alias::new(BillingAccount::table_name()))
		.columns(["id", "owner_identity", "label", "created_at"].map(Alias::new))
		.values_panic([
			IntoValue::into_value(account.id),
			IntoValue::into_value(account.owner_identity),
			IntoValue::into_value(account.label.clone()),
			IntoValue::into_value(account.created_at),
		])
		.to_owned();
	pool.atomic(&TenancyScope::NoTenant, async |tx| {
		execute(tx, &statement).await?;
		Ok(())
	})
	.await?;
	Ok(account)
}

#[derive(serde::Deserialize)]
pub struct AccountRow {
	pub id: Uuid,
	pub owner_identity: Uuid,
	pub label: String,
	pub created_at: DateTime<Utc>,
}

const ACCOUNT_COLUMNS: [&str; 4] = ["id", "owner_identity", "label", "created_at"];

pub async fn accounts_of(pool: &Pool, owner: Uuid) -> Result<Vec<AccountRow>> {
	let query = select(BillingAccount::table_name(), &ACCOUNT_COLUMNS)
		.and_where(Expr::col("owner_identity").eq(Expr::value(owner)))
		.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.to_owned();
	pool.atomic(&TenancyScope::NoTenant, async |tx| fetch(tx, &query).await)
		.await
}

/// The account `id`, held against concurrent attachments and Tier changes
/// until the transaction ends.
pub async fn lock_account(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
) -> Result<Option<AccountRow>> {
	let query = select(BillingAccount::table_name(), &ACCOUNT_COLUMNS)
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.lock(reinhardt::query::LockType::Update)
		.to_owned();
	Ok(fetch::<AccountRow>(tx, &query).await?.into_iter().next())
}

pub async fn account(pool: &Pool, id: Uuid) -> Result<Option<AccountRow>> {
	let query = select(BillingAccount::table_name(), &ACCOUNT_COLUMNS)
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.to_owned();
	Ok(pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			fetch::<AccountRow>(tx, &query).await
		})
		.await?
		.into_iter()
		.next())
}

/// A Tenant is covered by its attachment until it is purged: a deleted Tenant
/// in its grace period still holds its slot, and a purge frees it.
const NOT_PURGED: &str = "NOT EXISTS (SELECT 1 FROM tenant_deletions d \
	WHERE d.tenant = billing_attachments.tenant AND d.purged_at IS NOT NULL)";

#[derive(serde::Deserialize)]
struct TenantRow {
	tenant: String,
}

/// The Tenants `account` covers.
pub async fn covered_tenants(
	tx: &mut dyn TransactionExecutor,
	account: Uuid,
) -> Result<Vec<String>> {
	let query = select(BillingAttachment::table_name(), &["tenant"])
		.and_where(Expr::col("account_id").eq(Expr::value(account)))
		.and_where(Expr::cust(NOT_PURGED))
		.to_owned();
	Ok(fetch::<TenantRow>(tx, &query)
		.await?
		.into_iter()
		.map(|row| row.tenant)
		.collect())
}

#[derive(serde::Deserialize)]
struct AttachmentRow {
	account_id: Uuid,
}

pub async fn attached_account(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<Option<Uuid>> {
	let query = select(BillingAttachment::table_name(), &["account_id"])
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.to_owned();
	Ok(fetch::<AttachmentRow>(tx, &query)
		.await?
		.into_iter()
		.next()
		.map(|row| row.account_id))
}

#[derive(serde::Deserialize)]
struct ExistsRow {
	deleted: bool,
}

/// Whether the Tenant has a root record, and if so whether its Deletion is
/// recorded and not restored.
pub async fn tenant_deleted(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<Option<bool>> {
	let query = Query::select()
		.expr_as(
			Expr::cust(
				"EXISTS (SELECT 1 FROM tenant_deletions d \
				 WHERE d.tenant = tenants.name AND d.restored_at IS NULL)",
			),
			Alias::new("deleted"),
		)
		.from(Alias::new("tenants"))
		.and_where(Expr::col("name").eq(Expr::value(tenant)))
		.to_owned();
	Ok(fetch::<ExistsRow>(tx, &query)
		.await?
		.into_iter()
		.next()
		.map(|row| row.deleted))
}

#[derive(serde::Deserialize)]
pub struct ConsentRow {
	pub party: String,
	pub identity_id: Uuid,
}

/// The consents that are still valid for attaching `tenant` to `account`.
pub async fn consents(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	account: Uuid,
	now: DateTime<Utc>,
) -> Result<Vec<ConsentRow>> {
	let query = select(
		BillingAttachmentConsent::table_name(),
		&["party", "identity_id"],
	)
	.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
	.and_where(Expr::col("account_id").eq(Expr::value(account)))
	.and_where(Expr::col("expires_at").gt(Expr::value(now)))
	.to_owned();
	fetch(tx, &query).await
}

/// Records `party`'s consent, replacing its earlier one and removing the
/// consents of that Tenant that have expired.
pub async fn consent(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	account: Uuid,
	party: &str,
	identity: Uuid,
	now: DateTime<Utc>,
	expires_at: DateTime<Utc>,
) -> Result<()> {
	execute(
		tx,
		Query::delete()
			.from_table(Alias::new(BillingAttachmentConsent::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(
				Expr::col("expires_at")
					.lte(Expr::value(now))
					.or(Expr::col("account_id")
						.eq(Expr::value(account))
						.and(Expr::col("party").eq(Expr::value(party)))),
			),
	)
	.await?;
	execute(
		tx,
		Query::insert()
			.into_table(Alias::new(BillingAttachmentConsent::table_name()))
			.columns(
				[
					"id",
					"tenant",
					"account_id",
					"party",
					"identity_id",
					"created_at",
					"expires_at",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(Uuid::new_v4()),
				IntoValue::into_value(tenant.to_owned()),
				IntoValue::into_value(account),
				IntoValue::into_value(party.to_owned()),
				IntoValue::into_value(identity),
				IntoValue::into_value(now),
				IntoValue::into_value(expires_at),
			]),
	)
	.await?;
	Ok(())
}

/// Attaches `tenant` to `account`, moving it from another account, and
/// consumes every consent that named the Tenant.
pub async fn attach(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	account: Uuid,
	administrator: Uuid,
	owner: Uuid,
	now: DateTime<Utc>,
) -> Result<()> {
	execute(
		tx,
		Query::insert()
			.into_table(Alias::new(BillingAttachment::table_name()))
			.columns(
				[
					"tenant",
					"account_id",
					"attached_at",
					"consented_by_administrator",
					"consented_by_owner",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(tenant.to_owned()),
				IntoValue::into_value(account),
				IntoValue::into_value(now),
				IntoValue::into_value(administrator),
				IntoValue::into_value(owner),
			])
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.update_columns(
						[
							"account_id",
							"attached_at",
							"consented_by_administrator",
							"consented_by_owner",
						]
						.map(Alias::new),
					)
					.to_owned(),
			),
	)
	.await?;
	execute(
		tx,
		Query::delete()
			.from_table(Alias::new(BillingAttachmentConsent::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant))),
	)
	.await?;
	Ok(())
}

// What admission needs to know about a Tenant

pub struct TenantFacts {
	pub personal: bool,
	pub creator: Option<Uuid>,
	pub account: Option<Uuid>,
	/// The External Identity of each enabled Mapping.
	pub mappings: Vec<Uuid>,
}

#[derive(serde::Deserialize)]
struct RootRow {
	personal: bool,
	created_by_identity: Option<Uuid>,
	created_by_kind: String,
}

#[derive(serde::Deserialize)]
struct MappingRow {
	identity_id: Uuid,
}

/// `None` for a name with no root record.
pub async fn tenant_facts(pool: &Pool, tenant: &str) -> Result<Option<TenantFacts>> {
	let root = select(
		"tenants",
		&["personal", "created_by_identity", "created_by_kind"],
	)
	.and_where(Expr::col("name").eq(Expr::value(tenant)))
	.to_owned();
	let (root, account) = pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			let root = fetch::<RootRow>(tx, &root).await?.into_iter().next();
			let account = attached_account(tx, tenant).await?;
			Ok((root, account))
		})
		.await?;
	let Some(root) = root else {
		return Ok(None);
	};
	let mappings = select("dashboard_mappings", &["identity_id"])
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("enabled").eq(Expr::value(true)))
		.to_owned();
	let mappings = pool
		.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
			fetch::<MappingRow>(tx, &mappings).await
		})
		.await?;
	Ok(Some(TenantFacts {
		personal: root.personal,
		// A Deployment Administrator who created the Tenant is not its creator
		// for the Free Tier: only an External Identity creating a Personal
		// Tenant gets one.
		creator: (root.created_by_kind == "identity")
			.then_some(root.created_by_identity)
			.flatten(),
		account,
		mappings: mappings.into_iter().map(|row| row.identity_id).collect(),
	}))
}

// Usage

/// A Tenant Deletion's span: from the time it was recorded to the time it was
/// restored, if it was.
pub type Deletion = (DateTime<Utc>, Option<DateTime<Utc>>);

#[derive(serde::Deserialize)]
struct DeletionRow {
	tenant: String,
	deleted_at: DateTime<Utc>,
	restored_at: Option<DateTime<Utc>>,
}

pub async fn deletions(
	tx: &mut dyn TransactionExecutor,
	tenants: &[String],
) -> Result<BTreeMap<String, Vec<Deletion>>> {
	let mut found = BTreeMap::<String, Vec<Deletion>>::new();
	if tenants.is_empty() {
		return Ok(found);
	}
	let query = select("tenant_deletions", &["tenant", "deleted_at", "restored_at"])
		.and_where(Expr::col("tenant").is_in(tenants.iter().cloned()))
		.to_owned();
	for row in fetch::<DeletionRow>(tx, &query).await? {
		found
			.entry(row.tenant)
			.or_default()
			.push((row.deleted_at, row.restored_at));
	}
	Ok(found)
}

pub async fn record_pod_usage(
	tx: &mut dyn TransactionExecutor,
	operation: Uuid,
	tenant: &str,
	started_at: DateTime<Utc>,
	ended_at: DateTime<Utc>,
	millis: i64,
) -> Result<()> {
	execute(
		tx,
		Query::insert()
			.into_table(Alias::new(BillingPodUsage::table_name()))
			.columns(["operation", "tenant", "started_at", "ended_at", "millis"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(operation),
				IntoValue::into_value(tenant.to_owned()),
				IntoValue::into_value(started_at),
				IntoValue::into_value(ended_at),
				IntoValue::into_value(millis),
			])
			.on_conflict(
				OnConflict::column(Alias::new("operation"))
					.do_nothing()
					.to_owned(),
			),
	)
	.await?;
	Ok(())
}

/// The first sample of a Tenant for a UTC day stands: a pass repeated by
/// another replica records nothing new.
pub async fn record_storage_sample(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	sampled_at: DateTime<Utc>,
	bytes: i64,
) -> Result<()> {
	execute(
		tx,
		Query::insert()
			.into_table(Alias::new(BillingStorageSample::table_name()))
			.columns(["id", "tenant", "sampled_on", "sampled_at", "bytes"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(Uuid::new_v4()))
					.expr(Expr::value(tenant.to_owned()))
					.expr(date(sampled_at.date_naive()))
					.expr(Expr::value(sampled_at))
					.expr(Expr::value(bytes))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::columns(["tenant", "sampled_on"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			),
	)
	.await?;
	Ok(())
}

#[derive(serde::Deserialize)]
struct SumRow {
	total: i64,
}

/// The Execution Pod milliseconds metered for `tenant` since `since`.
pub async fn pod_millis_since(pool: &Pool, tenant: &str, since: DateTime<Utc>) -> Result<u64> {
	let query = Query::select()
		.expr_as(
			Expr::cust("COALESCE(SUM(millis), 0)::bigint"),
			Alias::new("total"),
		)
		.from(Alias::new(BillingPodUsage::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("ended_at").gte(Expr::value(since)))
		.to_owned();
	let rows = pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			fetch::<SumRow>(tx, &query).await
		})
		.await?;
	Ok(rows.first().map_or(0, |row| row.total.max(0) as u64))
}

#[derive(serde::Deserialize)]
struct BytesRow {
	bytes: i64,
}

/// The bytes of the latest sample of `tenant`, or none yet.
pub async fn latest_storage_bytes(pool: &Pool, tenant: &str) -> Result<u64> {
	let query = select(BillingStorageSample::table_name(), &["bytes"])
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.order_by(Alias::new("sampled_at"), reinhardt::query::Order::Desc)
		.limit(1)
		.to_owned();
	let rows = pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			fetch::<BytesRow>(tx, &query).await
		})
		.await?;
	Ok(rows.first().map_or(0, |row| row.bytes.max(0) as u64))
}

/// One day of one Tenant's usage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct UsageRow {
	pub tenant: String,
	pub date: NaiveDate,
	/// Execution Pod seconds of the Pods that stopped that UTC day.
	pub pod_seconds: u64,
	/// Bytes of the day's storage sample, if one was taken.
	pub storage_bytes: Option<u64>,
}

#[derive(serde::Deserialize)]
struct DayPods {
	tenant: String,
	day: NaiveDate,
	millis: i64,
}

#[derive(serde::Deserialize)]
struct DayStorage {
	tenant: String,
	sampled_on: NaiveDate,
	bytes: i64,
}

/// The usage of every Tenant from `from` to `to`, both included.
pub async fn usage(pool: &Pool, from: NaiveDate, to: NaiveDate) -> Result<Vec<UsageRow>> {
	let (start, end) = (
		from.and_hms_opt(0, 0, 0).expect("midnight").and_utc(),
		to.succ_opt()
			.ok_or_else(|| Error::Invalid("the period ends too late".into()))?
			.and_hms_opt(0, 0, 0)
			.expect("midnight")
			.and_utc(),
	);
	let pods = Query::select()
		.column(Alias::new("tenant"))
		.expr_as(
			Expr::cust("(ended_at AT TIME ZONE 'UTC')::date"),
			Alias::new("day"),
		)
		.expr_as(Expr::cust("SUM(millis)::bigint"), Alias::new("millis"))
		.from(Alias::new(BillingPodUsage::table_name()))
		.and_where(Expr::col("ended_at").gte(Expr::value(start)))
		.and_where(Expr::col("ended_at").lt(Expr::value(end)))
		.group_by_columns([Alias::new("tenant"), Alias::new("day")])
		.to_owned();
	let samples = select(
		BillingStorageSample::table_name(),
		&["tenant", "sampled_on", "bytes"],
	)
	.and_where(Expr::col("sampled_on").gte(date(from)))
	.and_where(Expr::col("sampled_on").lte(date(to)))
	.to_owned();
	let (pods, samples) = pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			Ok((
				fetch::<DayPods>(tx, &pods).await?,
				fetch::<DayStorage>(tx, &samples).await?,
			))
		})
		.await?;
	let mut days = BTreeMap::<(String, NaiveDate), (u64, Option<u64>)>::new();
	for pod in pods {
		days.entry((pod.tenant, pod.day)).or_default().0 =
			(pod.millis.max(0) as u64).div_ceil(1000);
	}
	for sample in samples {
		days.entry((sample.tenant, sample.sampled_on))
			.or_default()
			.1 = Some(sample.bytes.max(0) as u64);
	}
	Ok(days
		.into_iter()
		.map(|((tenant, date), (pod_seconds, storage_bytes))| UsageRow {
			tenant,
			date,
			pod_seconds,
			storage_bytes,
		})
		.collect())
}

/// Deletes what has outlived `cutoff`: metering records, and the attachments
/// of Tenants purged before it. Expired consents go too.
pub async fn sweep(pool: &Pool, cutoff: DateTime<Utc>, now: DateTime<Utc>) -> Result<()> {
	pool.atomic(&TenancyScope::NoTenant, async |tx| {
		execute(
			tx,
			Query::delete()
				.from_table(Alias::new(BillingPodUsage::table_name()))
				.and_where(Expr::col("ended_at").lt(Expr::value(cutoff))),
		)
		.await?;
		execute(
			tx,
			Query::delete()
				.from_table(Alias::new(BillingStorageSample::table_name()))
				.and_where(Expr::col("sampled_at").lt(Expr::value(cutoff))),
		)
		.await?;
		execute(
			tx,
			Query::delete()
				.from_table(Alias::new(BillingAttachment::table_name()))
				// The instant is formatted by chrono, never taken from a caller.
				.and_where(Expr::cust(format!(
					"EXISTS (SELECT 1 FROM tenant_deletions d WHERE d.tenant = \
					 billing_attachments.tenant AND d.purged_at < '{}'::timestamptz)",
					cutoff.to_rfc3339()
				))),
		)
		.await?;
		execute(
			tx,
			Query::delete()
				.from_table(Alias::new(BillingAttachmentConsent::table_name()))
				.and_where(Expr::col("expires_at").lte(Expr::value(now))),
		)
		.await?;
		Ok(())
	})
	.await
}
