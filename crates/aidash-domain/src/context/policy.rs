//! Versioned Context Policy, frozen with the immutable Agent version a Run pins.
use crate::{Error, Result, registry::EntityRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PRUNE_ONLY: &str = "prune-only/1";
pub const RECOVERY_V1: &str = "context-recovery/1";
pub const MIN_PRESERVE_RECENT: usize = 6;
pub const MAX_OVERFLOW_RETRIES: u8 = 4;
pub const MAX_SUMMARY_TOKENS: u32 = 4096;
const DEFAULT_SUMMARY_CALLS: u32 = 8;

/// Opt-in context recovery. An Agent version without one keeps the legacy
/// prune-only behavior: Jev pruning, no summary and no overflow retry.
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
		#[serde(default, skip_serializing_if = "Option::is_none")]
		summary: Option<SummaryPolicy>,
	},
}

/// The Summary Stage's explicit generation binding. There is no fallback model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryPolicy {
	/// Exact model Registry entry, pinned in the Run's admission closure.
	pub model: EntityRef,
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

impl ContextPolicy {
	pub fn validate(&self) -> Result<()> {
		let Self::RecoveryV1 {
			preserve_recent,
			overflow_retries,
			overflow_shrink,
			summary,
		} = self;
		if *preserve_recent < MIN_PRESERVE_RECENT
			|| *preserve_recent > 1000
			|| *overflow_retries > MAX_OVERFLOW_RETRIES
			|| !(0.5..=0.9).contains(overflow_shrink)
		{
			return Err(Error::Invalid(
				"Context Policy requires preserve_recent >= 6, overflow_retries <= 4 and overflow_shrink within 0.5..=0.9".into(),
			));
		}
		if let Some(summary) = summary
			&& (!(256..=MAX_SUMMARY_TOKENS).contains(&summary.max_tokens)
				|| !(1..=64).contains(&summary.call_budget)
				|| summary.model.id.trim().is_empty()
				|| summary.model.version.trim().is_empty())
		{
			return Err(Error::Invalid(
				"Context Policy summary requires an exact model, max_tokens within 256..=4096 and call_budget within 1..=64".into(),
			));
		}
		Ok(())
	}
}

/// The rules a Run actually applies, including the legacy default.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
	pub version: &'static str,
	pub preserve_recent: usize,
	pub overflow_retries: u8,
	pub overflow_shrink_permille: u16,
	pub summary: Option<SummaryPolicy>,
}

impl Effective {
	pub fn of(policy: Option<&ContextPolicy>) -> Self {
		match policy {
			None => Self {
				version: PRUNE_ONLY,
				preserve_recent: MIN_PRESERVE_RECENT,
				overflow_retries: 0,
				overflow_shrink_permille: 1000,
				summary: None,
			},
			Some(ContextPolicy::RecoveryV1 {
				preserve_recent,
				overflow_retries,
				overflow_shrink,
				summary,
			}) => Self {
				version: RECOVERY_V1,
				preserve_recent: *preserve_recent,
				overflow_retries: *overflow_retries,
				overflow_shrink_permille: (overflow_shrink * 1000.0).round() as u16,
				summary: summary.clone(),
			},
		}
	}

	pub fn is_legacy(&self) -> bool {
		self.version == PRUNE_ONLY
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
		assert!(effective.summary.is_none());
		for invalid in [
			serde_json::json!({"version":"context-recovery/1","preserve_recent":5}),
			serde_json::json!({"version":"context-recovery/1","overflow_retries":5}),
			serde_json::json!({"version":"context-recovery/1","overflow_shrink":0.95}),
			serde_json::json!({"version":"context-recovery/1","summary":{"model":{"id":"m","version":"1"},"call_budget":0}}),
			serde_json::json!({"version":"context-recovery/2"}),
		] {
			assert!(parse(invalid).is_err());
		}
	}

	#[test]
	fn legacy_policy_is_prune_only_without_overflow_retry() {
		let effective = Effective::of(None);
		assert!(effective.is_legacy());
		assert_eq!(effective.overflow_retries, 0);
		assert!(effective.summary_tokens(100_000).is_none());
	}

	#[test]
	fn summary_tokens_are_capped_by_window() {
		let policy = parse(
			serde_json::json!({"version":"context-recovery/1","summary":{"model":{"id":"m","version":"1"}}}),
		)
		.unwrap();
		let effective = Effective::of(Some(&policy));
		assert_eq!(effective.summary_tokens(16_000), Some(2000));
		assert_eq!(effective.summary_tokens(1_000_000), Some(4096));
	}
}
