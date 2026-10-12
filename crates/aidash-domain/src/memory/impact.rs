//! Authorized impact inspection for memory changes (Issue #127).
//!
//! One view serves as both the preview before a change and the post-change
//! view: a Run read is `fenced` once its revision is no longer the readable
//! revision of the Unit. The view is content-free, lists only Runs and derived
//! Units the requester may see, and is neither a lock nor a promise; every
//! mutation rechecks revisions and authority when it commits.
use super::{Bank, Bounds, Kind};
use crate::{Error, Result, RunControl, RunPhase, registry::EntityRef};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Visible Runs reported per Unit.
pub const RUNS_PER_UNIT: usize = 64;
/// Run read records examined per Unit, visible or not, before the scan stops.
pub const RUN_SCAN_PER_UNIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "MemoryImpactQuery")]
pub struct ImpactQuery {
	pub provider: EntityRef,
	pub bank: Bank,
	/// Units of `bank` to inspect, at most the pinned `max_candidates`.
	pub units: Vec<Uuid>,
}
impl ImpactQuery {
	pub fn validate(&self, bounds: &Bounds) -> Result<()> {
		self.bank.validate()?;
		let mut seen = std::collections::BTreeSet::new();
		if self.units.is_empty()
			|| self.units.len() > bounds.max_candidates
			|| self.units.iter().any(|id| id.is_nil() || !seen.insert(*id))
		{
			return Err(Error::Invalid(
				"memory impact needs distinct Units within the candidate bound".into(),
			));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "MemoryImpact")]
pub struct Impact {
	pub units: Vec<UnitImpact>,
	/// False when a declared bound stopped a scan before every dependent was
	/// examined. It covers only what the requester may see: hidden Runs and
	/// Units are never listed or counted, and a bound reached while skipping
	/// them also reports `false`.
	pub complete: bool,
	pub limits: Limits,
	/// When this view was taken. A later mutation rechecks everything itself.
	pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "MemoryImpactLimits")]
pub struct Limits {
	pub runs_per_unit: usize,
	pub run_scan_per_unit: usize,
	/// The pinned `max_graph_visits` that bounds derived-content traversal.
	pub derived_visits: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "MemoryUnitImpact")]
pub struct UnitImpact {
	pub id: Uuid,
	/// The revision a Run may still read; `None` once the Unit is deleted,
	/// withdrawn or retired, so every earlier read is fenced.
	pub readable_revision: Option<i64>,
	pub runs: Vec<AffectedRun>,
	pub derived: Vec<DerivedUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "MemoryAffectedRun")]
pub struct AffectedRun {
	pub id: Uuid,
	pub task_id: Uuid,
	pub phase: RunPhase,
	pub control: RunControl,
	pub read_revision: i64,
	/// The read no longer holds: the next boundary stops this Run. Otherwise a
	/// change to the Unit would fence it.
	pub fenced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "MemoryDerivedUnit")]
pub struct DerivedUnit {
	pub id: Uuid,
	pub bank: Bank,
	pub revision: i64,
	pub kind: Kind,
}

/// Whether a Run read of `read_revision` is already fenced.
pub fn fenced(read_revision: i64, readable_revision: Option<i64>) -> bool {
	readable_revision != Some(read_revision)
}

#[cfg(test)]
mod tests;
