// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Who may exist on Cloud, as pure rules over the facts the caller gathered.
//! The stricter of a Tier and the Tenancy Rules applies: these rules add the
//! Tier's limits and never relax what the Tenancy Rules decided.
use super::payments::{Status, Subscription};
use super::settings::{Settings, Tier};
use chrono::{DateTime, Utc};

/// What covers a Tenant's work.
#[derive(Debug, PartialEq, Eq)]
pub enum Cover<'a> {
	/// A Personal Tenant whose only Mapping is its Tenant Creator's.
	Free(&'a Tier),
	/// An active paid Subscription of the Billing Account it is attached to.
	Paid(&'a Tier),
}

/// Why nothing covers a Tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
	/// Its Billing Account is suspended.
	Suspended,
	/// It is not a Free Personal Tenant and has no active paid Subscription.
	Required,
}

impl Denied {
	/// The code an admission refusal carries.
	pub fn code(self) -> &'static str {
		match self {
			Self::Suspended => "billing_suspended",
			Self::Required => "billing_required",
		}
	}
}

/// What covers a Tenant whose Billing Account has `subscription`, if it has one.
/// A suspended account stops its Tenants even where they would be Free:
/// suspension covers every Tenant the account does.
pub fn cover<'a>(
	settings: &'a Settings,
	subscription: Option<&Subscription>,
	free_eligible: bool,
	now: DateTime<Utc>,
) -> Result<Cover<'a>, Denied> {
	if let Some(subscription) = subscription {
		match subscription.at(now) {
			Status::Suspended => return Err(Denied::Suspended),
			Status::Active | Status::PastDue => {
				if let Some(tier) = settings.paid(&subscription.tier) {
					return Ok(Cover::Paid(tier));
				}
			}
			Status::Canceled => {}
		}
	}
	if free_eligible {
		Ok(Cover::Free(settings.free()))
	} else {
		Err(Denied::Required)
	}
}

/// A Free Tenant has used what the Free Tier includes. Nothing bills it, so
/// the included usage is also its ceiling.
pub fn free_exhausted(tier: &Tier, month_pod_seconds: u64, storage_bytes: u64) -> bool {
	month_pod_seconds >= tier.pod_seconds || storage_bytes > tier.storage_bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitRefusal {
	/// The account has no active paid Subscription.
	NoActiveSubscription,
	TenantLimit {
		limit: u32,
	},
	NodeLimit {
		limit: u32,
	},
}

impl LimitRefusal {
	pub fn code(self) -> &'static str {
		match self {
			Self::NoActiveSubscription => "subscription_required",
			Self::TenantLimit { .. } => "tier_tenant_limit",
			Self::NodeLimit { .. } => "tier_node_limit",
		}
	}
}

/// Whether a Tenant with `tenant_nodes` Nodes may be attached to an account
/// that already covers `covered_tenants` other Tenants, deleted ones in their
/// grace period included.
pub fn attach_refusal(
	settings: &Settings,
	subscription: Option<&Subscription>,
	covered_tenants: u32,
	tenant_nodes: u32,
	now: DateTime<Utc>,
) -> Option<LimitRefusal> {
	let tier = subscription
		.filter(|subscription| subscription.at(now) == Status::Active)
		.and_then(|subscription| settings.paid(&subscription.tier));
	let Some(tier) = tier else {
		return Some(LimitRefusal::NoActiveSubscription);
	};
	if covered_tenants >= tier.tenants {
		return Some(LimitRefusal::TenantLimit {
			limit: tier.tenants,
		});
	}
	if tenant_nodes > tier.nodes {
		return Some(LimitRefusal::NodeLimit { limit: tier.nodes });
	}
	None
}

/// Whether an account covering `covered_tenants` Tenants, the one with the
/// most Nodes having `most_nodes`, may change to `tier`. Every limit counts,
/// the Node limit included.
pub fn tier_change_refusal(
	tier: &Tier,
	covered_tenants: u32,
	most_nodes: u32,
) -> Option<LimitRefusal> {
	if covered_tenants > tier.tenants {
		return Some(LimitRefusal::TenantLimit {
			limit: tier.tenants,
		});
	}
	if most_nodes > tier.nodes {
		return Some(LimitRefusal::NodeLimit { limit: tier.nodes });
	}
	None
}

#[cfg(test)]
mod tests {
	use super::super::settings::sandbox_defaults;
	use super::*;
	use chrono::Duration;
	use rstest::{fixture, rstest};

	#[fixture]
	fn settings() -> Settings {
		Settings::from_value(sandbox_defaults()).unwrap()
	}

	fn subscription(status: Status) -> Subscription {
		Subscription {
			tier: "team".into(),
			status,
			grace_ends_at: None,
		}
	}

	#[rstest]
	fn a_free_personal_tenant_is_covered_without_a_subscription(settings: Settings) {
		let now = Utc::now();
		assert_eq!(
			cover(&settings, None, true, now),
			Ok(Cover::Free(settings.free()))
		);
		assert_eq!(cover(&settings, None, false, now), Err(Denied::Required));
	}

	#[rstest]
	#[case(Status::Active, true, Ok(true))]
	#[case(Status::PastDue, false, Ok(true))]
	#[case(Status::Suspended, true, Err(Denied::Suspended))]
	#[case(Status::Canceled, true, Ok(false))]
	#[case(Status::Canceled, false, Err(Denied::Required))]
	fn a_subscription_covers_by_its_status(
		settings: Settings,
		#[case] status: Status,
		#[case] free_eligible: bool,
		#[case] expected: Result<bool, Denied>,
	) {
		let covered = cover(
			&settings,
			Some(&subscription(status)),
			free_eligible,
			Utc::now(),
		);
		let paid = covered.map(|covered| matches!(covered, Cover::Paid(_)));
		assert_eq!(paid, expected);
	}

	#[rstest]
	fn the_end_of_the_grace_period_suspends_without_a_report(settings: Settings) {
		let now = Utc::now();
		let mut past_due = subscription(Status::PastDue);
		past_due.grace_ends_at = Some(now + Duration::days(14));
		assert!(cover(&settings, Some(&past_due), false, now).is_ok());
		assert_eq!(
			cover(&settings, Some(&past_due), false, now + Duration::days(15)),
			Err(Denied::Suspended)
		);
	}

	#[rstest]
	fn an_unknown_tier_covers_nothing_and_the_free_tier_is_never_a_subscription(
		settings: Settings,
	) {
		let now = Utc::now();
		for tier in ["gone", "free"] {
			let mut subscription = subscription(Status::Active);
			subscription.tier = tier.into();
			assert_eq!(
				cover(&settings, Some(&subscription), false, now),
				Err(Denied::Required)
			);
		}
	}

	#[rstest]
	fn the_free_tier_is_also_the_ceiling_of_a_free_tenant(settings: Settings) {
		let free = settings.free();
		assert!(!free_exhausted(
			free,
			free.pod_seconds - 1,
			free.storage_bytes
		));
		assert!(free_exhausted(free, free.pod_seconds, 0));
		assert!(free_exhausted(free, 0, free.storage_bytes + 1));
	}

	#[rstest]
	#[case(None, 0, 1, Some(LimitRefusal::NoActiveSubscription))]
	#[case(Some(Status::PastDue), 0, 1, Some(LimitRefusal::NoActiveSubscription))]
	#[case(Some(Status::Active), 4, 10, None)]
	#[case(Some(Status::Active), 5, 1, Some(LimitRefusal::TenantLimit { limit: 5 }))]
	#[case(Some(Status::Active), 0, 11, Some(LimitRefusal::NodeLimit { limit: 10 }))]
	fn an_attachment_needs_an_active_subscription_with_room(
		settings: Settings,
		#[case] status: Option<Status>,
		#[case] covered: u32,
		#[case] nodes: u32,
		#[case] expected: Option<LimitRefusal>,
	) {
		let subscription = status.map(subscription);
		assert_eq!(
			attach_refusal(&settings, subscription.as_ref(), covered, nodes, Utc::now()),
			expected
		);
	}

	#[rstest]
	fn a_tier_change_is_refused_on_any_limit_including_nodes(settings: Settings) {
		let team = settings.paid("team").unwrap();
		assert_eq!(tier_change_refusal(team, 5, 10), None);
		assert_eq!(
			tier_change_refusal(team, 6, 1),
			Some(LimitRefusal::TenantLimit { limit: 5 })
		);
		assert_eq!(
			tier_change_refusal(team, 1, 11),
			Some(LimitRefusal::NodeLimit { limit: 10 })
		);
	}
}
