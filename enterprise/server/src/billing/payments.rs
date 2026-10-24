//! The payment state of a Billing Account, behind one port. The rules read
//! only this port and never name a payment processor, so the processor that
//! fills it can change without touching them. Aidash holds the state itself:
//! admission never waits on the processor.
use aidash_server::database::native::Pool;
use aidash_server::{Error, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
	Active,
	/// A payment failed; the grace period runs until `grace_ends_at`.
	PastDue,
	Suspended,
	Canceled,
}

impl Status {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Active => "active",
			Self::PastDue => "past_due",
			Self::Suspended => "suspended",
			Self::Canceled => "canceled",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		Ok(match value {
			"active" => Self::Active,
			"past_due" => Self::PastDue,
			"suspended" => Self::Suspended,
			"canceled" => Self::Canceled,
			other => {
				return Err(Error::Invalid(format!(
					"unknown Subscription status {other}"
				)));
			}
		})
	}
}

/// A Billing Account's contract for one Tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
	pub tier: String,
	pub status: Status,
	pub grace_ends_at: Option<DateTime<Utc>>,
}

impl Subscription {
	/// What the Subscription is at `now`: a past-due one whose grace period
	/// has ended is suspended, whether or not the processor has said so yet.
	pub fn at(&self, now: DateTime<Utc>) -> Status {
		match (self.status, self.grace_ends_at) {
			(Status::PastDue, Some(ends)) if ends <= now => Status::Suspended,
			(status, _) => status,
		}
	}
}

#[async_trait]
pub trait PaymentState: Send + Sync {
	/// The Subscription of `account`, from state Aidash already holds.
	async fn subscription(&self, account: Uuid) -> Result<Option<Subscription>>;

	/// Moves the Subscription to `tier`. The caller has already checked the
	/// limits while holding the covered Tenants' capacity locks. Success must
	/// publish the new locally readable Subscription before returning, so a
	/// waiting Node creation observes its new limit.
	async fn change_tier(&self, account: Uuid, tier: &str) -> Result<()>;
}

/// Reads the Subscriptions that the processor's reports left in
/// `billing_subscriptions`. A build without a processor can change no Tier.
pub struct Mirror {
	pub pool: Pool,
}

#[async_trait]
impl PaymentState for Mirror {
	async fn subscription(&self, account: Uuid) -> Result<Option<Subscription>> {
		super::store::subscription(&self.pool, account).await
	}

	async fn change_tier(&self, _: Uuid, _: &str) -> Result<()> {
		Err(Error::Conflict("payment_processor_unavailable".into()))
	}
}

/// An in-memory port for tests.
#[derive(Default)]
pub struct FakePayments {
	subscriptions: Mutex<BTreeMap<Uuid, Subscription>>,
	changes: Mutex<Vec<(Uuid, String)>>,
}

impl FakePayments {
	pub fn set(&self, account: Uuid, subscription: Subscription) {
		self.subscriptions
			.lock()
			.unwrap()
			.insert(account, subscription);
	}

	/// The Tier changes the port was asked for, in order.
	pub fn changes(&self) -> Vec<(Uuid, String)> {
		self.changes.lock().unwrap().clone()
	}
}

#[async_trait]
impl PaymentState for FakePayments {
	async fn subscription(&self, account: Uuid) -> Result<Option<Subscription>> {
		Ok(self.subscriptions.lock().unwrap().get(&account).cloned())
	}

	async fn change_tier(&self, account: Uuid, tier: &str) -> Result<()> {
		self.changes.lock().unwrap().push((account, tier.into()));
		if let Some(subscription) = self.subscriptions.lock().unwrap().get_mut(&account) {
			subscription.tier = tier.into();
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use chrono::Duration;

	#[test]
	fn a_past_due_subscription_is_suspended_once_its_grace_period_ends() {
		let now = Utc::now();
		let past_due = |ends: Option<DateTime<Utc>>| Subscription {
			tier: "team".into(),
			status: Status::PastDue,
			grace_ends_at: ends,
		};
		assert_eq!(
			past_due(Some(now + Duration::days(1))).at(now),
			Status::PastDue
		);
		assert_eq!(past_due(Some(now)).at(now), Status::Suspended);
		assert_eq!(past_due(None).at(now), Status::PastDue);
	}

	#[test]
	fn a_status_round_trips_through_its_stored_text() {
		for status in [
			Status::Active,
			Status::PastDue,
			Status::Suspended,
			Status::Canceled,
		] {
			assert_eq!(Status::parse(status.as_str()).unwrap(), status);
		}
		assert!(Status::parse("paused").is_err());
	}
}
