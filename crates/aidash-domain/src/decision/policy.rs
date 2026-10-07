//! Shared exact pins for ordinary, generated and cross-Node decision approval.
use super::*;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAllowance {
	pub decider: DeciderPin,
	pub max_calls_per_run: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerationAllowance {
	pub decider: DeciderPin,
	pub calls_per_agent: u64,
	pub call_budget: u64,
}
pub type ExecutionDecisions = BTreeMap<Hook, ExecutionAllowance>;
pub type GenerationDecisions = BTreeMap<Hook, GenerationAllowance>;
impl ExecutionAllowance {
	pub fn remaining(&self, current: &Self, consumed: u64) -> Result<u64> {
		self.decider.validate()?;
		current.decider.validate()?;
		if self.decider != current.decider {
			return Err(Error::Invalid(
				"current decision policy changed the pinned Decider".into(),
			));
		}
		Ok(self
			.max_calls_per_run
			.min(current.max_calls_per_run)
			.saturating_sub(consumed))
	}
}
impl GenerationAllowance {
	pub fn validate(&self) -> Result<()> {
		self.decider.validate()
	}
}

/// This controls retained state only. It grants no disclosure or invocation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StateRetention {
	pub enabled: bool,
	pub days: u8,
}
impl Default for StateRetention {
	fn default() -> Self {
		Self {
			enabled: false,
			days: 7,
		}
	}
}
impl StateRetention {
	pub fn expires_at(
		&self,
		now: DateTime<Utc>,
		source_expiry: Option<DateTime<Utc>>,
	) -> Result<Option<DateTime<Utc>>> {
		if !(1..=30).contains(&self.days) {
			return Err(Error::Invalid(
				"decision state retention must be 1..30 days".into(),
			));
		}
		if !self.enabled {
			return Ok(None);
		}
		let deadline = now
			.checked_add_signed(Duration::days(i64::from(self.days)))
			.ok_or_else(|| Error::Invalid("decision retention time overflow".into()))?;
		let expiry = source_expiry.map_or(deadline, |source| source.min(deadline));
		if expiry <= now {
			return Err(Error::Invalid(
				"decision source retention already expired".into(),
			));
		}
		Ok(Some(expiry))
	}
}
