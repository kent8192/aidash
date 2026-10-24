//! The use cases of Billing Accounts. Callers have authenticated the External
//! Identity, or the Tenant Administrator's Mapping, that each acts as.
use super::models::BillingAccount;
use super::nodes::NodeCount;
use super::payments::PaymentState;
use super::rules;
use super::settings::{Settings, Tier};
use super::store::{self, UsageRow};
use aidash_application::tenancy::TenancyScope;
use aidash_server::database::native::Pool;
use aidash_server::{Error, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

/// How long one party's consent waits for the other's.
const CONSENT_DAYS: i64 = 7;
const MAX_USAGE_DAYS: i64 = 366;
const LABEL_MAX: usize = 64;

pub struct Billing {
	pub pool: Pool,
	pub settings: Arc<Settings>,
	pub payments: Arc<dyn PaymentState>,
	pub nodes: Arc<dyn NodeCount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Party {
	/// A Tenant Administrator whom the Tenant's policy allows `billing.attach`.
	TenantAdministrator,
	/// The External Identity that owns the receiving Billing Account.
	AccountOwner,
}

impl Party {
	fn as_str(self) -> &'static str {
		match self {
			Self::TenantAdministrator => "tenant_administrator",
			Self::AccountOwner => "account_owner",
		}
	}

	fn other(self) -> &'static str {
		match self {
			Self::TenantAdministrator => Self::AccountOwner.as_str(),
			Self::AccountOwner => Self::TenantAdministrator.as_str(),
		}
	}
}

/// What a consent did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Consented {
	/// Recorded; the Tenant is attached once the other party consents too.
	AwaitingOtherParty,
	Attached,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AccountView {
	pub id: Uuid,
	pub label: String,
	pub created_at: DateTime<Utc>,
	pub tier: Option<String>,
	pub status: Option<String>,
	pub grace_ends_at: Option<DateTime<Utc>>,
	pub tenants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TierView {
	pub name: String,
	pub tenants: u32,
	pub nodes: u32,
	pub pod_seconds: u64,
	pub storage_bytes: u64,
}

fn refused(refusal: rules::LimitRefusal) -> Error {
	Error::Conflict(refusal.code().into())
}

fn count(tenants: usize) -> u32 {
	u32::try_from(tenants).unwrap_or(u32::MAX)
}

impl Billing {
	/// The Tiers a Billing Account can subscribe to, Free apart.
	pub fn tiers(&self) -> Vec<TierView> {
		self.settings
			.tiers
			.iter()
			.filter(|(name, _)| self.settings.paid(name).is_some())
			.map(|(name, tier)| TierView {
				name: name.clone(),
				tenants: tier.tenants,
				nodes: tier.nodes,
				pod_seconds: tier.pod_seconds,
				storage_bytes: tier.storage_bytes,
			})
			.collect()
	}

	pub async fn create_account(&self, owner: Uuid, label: &str) -> Result<BillingAccount> {
		let label = label.trim();
		if label.is_empty()
			|| label.chars().count() > LABEL_MAX
			|| label.chars().any(char::is_control)
		{
			return Err(Error::Invalid(
				"a Billing Account label needs 1 to 64 characters without control characters"
					.into(),
			));
		}
		store::create_account(&self.pool, owner, label).await
	}

	/// The accounts `owner` owns, with their Subscriptions and Tenants.
	pub async fn accounts(&self, owner: Uuid) -> Result<Vec<AccountView>> {
		let mut views = Vec::new();
		for account in store::accounts_of(&self.pool, owner).await? {
			let subscription = self.payments.subscription(account.id).await?;
			let tenants = self
				.pool
				.atomic(&TenancyScope::NoTenant, async |tx| {
					store::covered_tenants(tx, account.id).await
				})
				.await?;
			views.push(AccountView {
				id: account.id,
				label: account.label,
				created_at: account.created_at,
				tier: subscription.as_ref().map(|s| s.tier.clone()),
				status: subscription.as_ref().map(|s| s.status.as_str().into()),
				grace_ends_at: subscription.and_then(|s| s.grace_ends_at),
				tenants,
			});
		}
		Ok(views)
	}

	async fn owned(&self, owner: Uuid, account: Uuid) -> Result<store::AccountRow> {
		store::account(&self.pool, account)
			.await?
			.filter(|row| row.owner_identity == owner)
			.ok_or_else(|| Error::NotFound("billing account".into()))
	}

	/// Moves the account's Subscription to `tier`, which the account's Tenants
	/// must fit: the Tenant limit and the Node limit both count.
	pub async fn change_tier(&self, owner: Uuid, account: Uuid, tier: &str) -> Result<()> {
		self.owned(owner, account).await?;
		let target: &Tier = self
			.settings
			.paid(tier)
			.ok_or_else(|| Error::Invalid("unknown Tier".into()))?;
		if self.payments.subscription(account).await?.is_none() {
			return Err(refused(rules::LimitRefusal::NoActiveSubscription));
		}
		let mut tx = self.pool.begin(&TenancyScope::NoTenant).await?;
		let row = store::lock_account(&mut *tx, account)
			.await?
			.ok_or_else(|| Error::NotFound("billing account".into()))?;
		if row.owner_identity != owner {
			return Err(Error::NotFound("billing account".into()));
		}
		let mut tenants = store::covered_tenants(&mut *tx, account).await?;
		tenants.sort();
		for tenant in &tenants {
			aidash_server::authorization::services::nodes::lock_capacity(&mut *tx, tenant).await?;
		}
		let mut most_nodes = 0;
		for tenant in &tenants {
			most_nodes = most_nodes.max(self.nodes.nodes(tenant).await?);
		}
		if let Some(refusal) = rules::tier_change_refusal(target, count(tenants.len()), most_nodes)
		{
			return Err(refused(refusal));
		}
		self.payments.change_tier(account, tier).await?;
		tx.commit().await?;
		Ok(())
	}

	/// Records `party`'s consent, by `identity`, to attaching `tenant` to
	/// `account`, and attaches it once the other party has consented too.
	/// The caller has authenticated `identity` as `party`; for the account's
	/// owner this checks that it owns the account.
	pub async fn consent(
		&self,
		party: Party,
		identity: Uuid,
		tenant: &str,
		account: Uuid,
	) -> Result<Consented> {
		self.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				let row = store::lock_account(tx, account)
					.await?
					.ok_or_else(|| Error::NotFound("billing account".into()))?;
				if party == Party::AccountOwner && row.owner_identity != identity {
					return Err(Error::NotFound("billing account".into()));
				}
				// Every consent and transition shares the same Tenant serialization,
				// after the receiving account lock. A competing move must not reuse
				// consents that another completed move has already consumed.
				aidash_server::authorization::services::nodes::lock_capacity(tx, tenant).await?;
				if store::tenant_deleted(tx, tenant).await? != Some(false) {
					return Err(Error::NotFound("tenant".into()));
				}
				if store::attached_account(tx, tenant).await? == Some(account) {
					// The caller's authority and Tenant liveness have been checked.
					// No new capacity or consent is needed for an unchanged attachment.
					return Ok(Consented::Attached);
				}
				let now = store::database_now(tx).await?;
				store::consent(
					tx,
					tenant,
					account,
					party.as_str(),
					identity,
					now,
					now + Duration::days(CONSENT_DAYS),
				)
				.await?;
				let consents = store::consents(tx, tenant, account, now).await?;
				let Some(other) = consents.iter().find(|c| c.party == party.other()) else {
					return Ok(Consented::AwaitingOtherParty);
				};
				let (administrator, owner) = match party {
					Party::TenantAdministrator => (identity, other.identity_id),
					Party::AccountOwner => (other.identity_id, identity),
				};
				let subscription = self.payments.subscription(account).await?;
				let nodes = self.nodes.nodes(tenant).await?;
				let covered = store::covered_tenants(tx, account).await?;
				let others = covered.iter().filter(|covered| *covered != tenant).count();
				if let Some(refusal) = rules::attach_refusal(
					&self.settings,
					subscription.as_ref(),
					count(others),
					nodes,
					now,
				) {
					return Err(refused(refusal));
				}
				store::attach(tx, tenant, account, administrator, owner, now).await?;
				Ok(Consented::Attached)
			})
			.await
	}

	/// The usage of every Tenant over `from..=to`, for a Deployment
	/// Administrator.
	pub async fn usage(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<UsageRow>> {
		if to < from || (to - from).num_days() >= MAX_USAGE_DAYS {
			return Err(Error::Invalid(
				"a usage report covers one to 366 days, oldest first".into(),
			));
		}
		store::usage(&self.pool, from, to).await
	}

	/// Historical facts of an owned account over an explicit half-open period.
	pub async fn account_usage(
		&self,
		owner: Uuid,
		account: Uuid,
		starts_at: DateTime<Utc>,
		ends_at: DateTime<Utc>,
	) -> Result<super::AccountUsage> {
		self.owned(owner, account).await?;
		store::account_usage(&self.pool, account, starts_at, ends_at).await
	}
}

/// The report as CSV: a header, then one line per Tenant and day.
pub fn csv(rows: &[UsageRow]) -> String {
	fn cell(value: &str) -> String {
		if value.contains([',', '"', '\n', '\r']) {
			format!("\"{}\"", value.replace('"', "\"\""))
		} else {
			value.to_owned()
		}
	}
	let mut text = String::from("tenant,date,pod_seconds,storage_bytes\r\n");
	for row in rows {
		text.push_str(&format!(
			"{},{},{},{}\r\n",
			cell(&row.tenant),
			row.date,
			row.pod_seconds,
			row.storage_bytes
				.map_or_else(String::new, |bytes| bytes.to_string()),
		));
	}
	text
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_csv_has_a_header_one_line_per_day_and_an_empty_cell_for_a_missing_sample() {
		let day = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
		let rows = [
			UsageRow {
				tenant: "calm-heron-00000000".into(),
				date: day,
				pod_seconds: 90,
				storage_bytes: Some(4096),
			},
			UsageRow {
				tenant: "odd,\"name".into(),
				date: day,
				pod_seconds: 0,
				storage_bytes: None,
			},
		];
		assert_eq!(
			csv(&rows),
			"tenant,date,pod_seconds,storage_bytes\r\n\
			 calm-heron-00000000,2026-10-19,90,4096\r\n\
			 \"odd,\"\"name\",2026-10-19,0,\r\n"
		);
	}
}
