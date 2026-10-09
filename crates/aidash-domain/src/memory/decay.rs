//! Retention can demote relevance; it never creates a retrieval hit or changes validity.
use super::{Decay, Learning, Unit};
use chrono::{DateTime, Utc};

pub fn retention_score(
	decay: &Decay,
	learned_at: DateTime<Utc>,
	last_delivered_at: Option<DateTime<Utc>>,
	deliveries: u64,
	activated_at: DateTime<Utc>,
	reactivated_at: Option<DateTime<Utc>>,
	as_of: DateTime<Utc>,
) -> f64 {
	let anchor = last_delivered_at
		.unwrap_or(learned_at)
		.max(activated_at)
		.max(reactivated_at.unwrap_or(activated_at));
	let elapsed = as_of.signed_duration_since(anchor);
	let seconds = (elapsed.num_seconds() as f64
		+ f64::from(elapsed.subsec_nanos()) / 1_000_000_000.0)
		.max(0.0);
	let days = seconds / 86_400.0;
	let half_life = f64::from(decay.half_life_days) * (1.0 + (deliveries as f64).ln_1p());
	// Preserve the mathematical open lower bound even after floating point underflow.
	(-days / half_life).exp2().max(f64::MIN_POSITIVE)
}

pub fn prior(decay: &Decay, retention_score: f64) -> f64 {
	let floor = f64::from(decay.prior_floor_millionths) / 1_000_000.0;
	floor + (1.0 - floor) * retention_score
}

pub fn exempt(unit: &Unit, decay: &Decay, pinned: bool, live_support: bool) -> bool {
	let dormancy = decay.dormancy.as_ref();
	pinned
		|| live_support
		|| match unit.content.learning {
			Learning::Preference => dormancy.is_none_or(|d| !d.include_preferences),
			Learning::Procedure => dormancy.is_none_or(|d| !d.include_procedures),
			_ => false,
		}
}
