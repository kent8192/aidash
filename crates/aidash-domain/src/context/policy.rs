//! Versioned Context Policy, frozen with the immutable Agent version a Run pins.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const RECOVERY_V1: &str = "context-recovery/1";
pub const MIN_PRESERVE_RECENT: usize = 6;
pub const MAX_OVERFLOW_RETRIES: u8 = 4;
pub const MAX_SUMMARY_TOKENS: u32 = 4096;
const DEFAULT_SUMMARY_CALLS: u32 = 8;

/// Context recovery settings, with defaults for every Agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "version", deny_unknown_fields)]
pub enum ContextPolicy {
	#[serde(rename = "context-recovery/1")]
	RecoveryV1 {
		#[serde(default = "preserve_recent")]
		preserve_recent: usize,
		#[serde(default = "overflow_retries")]
		overflow_retries: u8,
		#[serde(default = "overflow_shrink")]
		overflow_shrink: f64,
		#[serde(default = "offload_target")]
		offload_target: f64,
		#[serde(default = "protected_tail_fraction")]
		protected_tail_fraction: f64,
		#[serde(default = "reread_immunity_steps")]
		reread_immunity_steps: u32,
		#[serde(default = "reread_summary_threshold")]
		reread_summary_threshold: u32,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		summary: Option<SummaryPolicy>,
	},
}

/// Limits for the Summary Stage, which uses the Agent's own model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryPolicy {
	/// Upper bound on summary output, further capped at an eighth of the window.
	#[serde(default = "summary_tokens")]
	pub max_tokens: u32,
	/// Summary requests allowed per Run, including failed attempts.
	#[serde(default = "summary_calls")]
	pub call_budget: u32,
}

fn preserve_recent() -> usize {
	MIN_PRESERVE_RECENT
}
fn overflow_retries() -> u8 {
	2
}
fn overflow_shrink() -> f64 {
	0.75
}
fn summary_tokens() -> u32 {
	MAX_SUMMARY_TOKENS
}
fn summary_calls() -> u32 {
	DEFAULT_SUMMARY_CALLS
}
fn offload_target() -> f64 {
	0.7
}
fn protected_tail_fraction() -> f64 {
	0.25
}
fn reread_immunity_steps() -> u32 {
	3
}
fn reread_summary_threshold() -> u32 {
	3
}

impl Default for SummaryPolicy {
	fn default() -> Self {
		Self {
			max_tokens: summary_tokens(),
			call_budget: summary_calls(),
		}
	}
}

impl ContextPolicy {
	pub fn validate(&self) -> Result<()> {
		let Self::RecoveryV1 {
			preserve_recent,
			overflow_retries,
			overflow_shrink,
			offload_target,
			protected_tail_fraction,
			reread_immunity_steps,
			reread_summary_threshold,
			summary,
		} = self;
		if *preserve_recent < MIN_PRESERVE_RECENT
			|| *preserve_recent > 1000
			|| *overflow_retries > MAX_OVERFLOW_RETRIES
			|| !(0.5..=0.9).contains(overflow_shrink)
			|| !(0.5..=0.9).contains(offload_target)
			|| !(0.05..=0.5).contains(protected_tail_fraction)
			|| !(1..=100).contains(reread_immunity_steps)
			|| !(2..=100).contains(reread_summary_threshold)
		{
			return Err(Error::Invalid(
				"Context Policy requires preserve_recent within 6..=1000, overflow_retries <= 4, overflow_shrink and offload_target within 0.5..=0.9, protected_tail_fraction within 0.05..=0.5, reread_immunity_steps within 1..=100 and reread_summary_threshold within 2..=100".into(),
			));
		}
		if let Some(summary) = summary
			&& (!(256..=MAX_SUMMARY_TOKENS).contains(&summary.max_tokens)
				|| !(1..=64).contains(&summary.call_budget))
		{
			return Err(Error::Invalid(
				"Context Policy summary requires max_tokens within 256..=4096 and call_budget within 1..=64".into(),
			));
		}
		Ok(())
	}
}

/// The rules a Run actually applies.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
	pub version: &'static str,
	pub preserve_recent: usize,
	pub overflow_retries: u8,
	pub overflow_shrink_permille: u16,
	pub offload_target_permille: u16,
	pub protected_tail_permille: u16,
	pub reread_immunity_steps: u32,
	pub reread_summary_threshold: u32,
	pub summary: Option<SummaryPolicy>,
}

impl Effective {
	pub fn of(policy: Option<&ContextPolicy>) -> Self {
		match policy {
			None => Self {
				version: RECOVERY_V1,
				preserve_recent: MIN_PRESERVE_RECENT,
				overflow_retries: overflow_retries(),
				overflow_shrink_permille: (overflow_shrink() * 1000.0) as u16,
				offload_target_permille: (offload_target() * 1000.0) as u16,
				protected_tail_permille: (protected_tail_fraction() * 1000.0) as u16,
				reread_immunity_steps: reread_immunity_steps(),
				reread_summary_threshold: reread_summary_threshold(),
				summary: Some(SummaryPolicy::default()),
			},
			Some(ContextPolicy::RecoveryV1 {
				preserve_recent,
				overflow_retries,
				overflow_shrink,
				offload_target,
				protected_tail_fraction,
				reread_immunity_steps,
				reread_summary_threshold,
				summary,
			}) => Self {
				version: RECOVERY_V1,
				preserve_recent: *preserve_recent,
				overflow_retries: *overflow_retries,
				overflow_shrink_permille: (overflow_shrink * 1000.0).round() as u16,
				offload_target_permille: (offload_target * 1000.0).round() as u16,
				protected_tail_permille: (protected_tail_fraction * 1000.0).round() as u16,
				reread_immunity_steps: *reread_immunity_steps,
				reread_summary_threshold: *reread_summary_threshold,
				summary: Some(summary.clone().unwrap_or_default()),
			},
		}
	}

	/// Summary output bound for a given request window.
	pub fn summary_tokens(&self, window: usize) -> Option<u32> {
		self.summary.as_ref().map(|summary| {
			summary
				.max_tokens
				.min(u32::try_from(window / 8).unwrap_or(u32::MAX))
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn parse(value: serde_json::Value) -> Result<ContextPolicy> {
		let policy: ContextPolicy = serde_json::from_value(value)?;
		policy.validate()?;
		Ok(policy)
	}

	#[test]
	fn defaults_and_bounds() {
		let policy = parse(serde_json::json!({"version":"context-recovery/1"})).unwrap();
		let effective = Effective::of(Some(&policy));
		assert_eq!(effective.preserve_recent, 6);
		assert_eq!(effective.overflow_retries, 2);
		assert_eq!(effective.overflow_shrink_permille, 750);
		assert!(effective.summary.is_some());
		assert_eq!(effective.offload_target_permille, 700);
		for invalid in [
			serde_json::json!({"version":"context-recovery/1","preserve_recent":5}),
			serde_json::json!({"version":"context-recovery/1","overflow_retries":5}),
			serde_json::json!({"version":"context-recovery/1","overflow_shrink":0.95}),
			serde_json::json!({"version":"context-recovery/1","summary":{"call_budget":0}}),
			serde_json::json!({"version":"context-recovery/1","summary":{"model":{"id":"m","version":"1"}}}),
			serde_json::json!({"version":"context-recovery/1","offload_target":0.99}),
			serde_json::json!({"version":"context-recovery/2"}),
		] {
			assert!(parse(invalid).is_err());
		}
	}

	#[test]
	fn absent_policy_enables_recovery_and_summary() {
		let effective = Effective::of(None);
		assert_eq!(effective.version, RECOVERY_V1);
		assert_eq!(effective.overflow_retries, 2);
		assert!(effective.summary_tokens(100_000).is_some());
	}

	#[test]
	fn summary_tokens_are_capped_by_window() {
		let policy =
			parse(serde_json::json!({"version":"context-recovery/1","summary":{}})).unwrap();
		let effective = Effective::of(Some(&policy));
		assert_eq!(effective.summary_tokens(16_000), Some(2000));
		assert_eq!(effective.summary_tokens(1_000_000), Some(4096));
	}
}
