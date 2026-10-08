//! External monotone fences prevent a canonical snapshot from reviving withdrawn content.
use super::{Bank, Unit};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fence {
	pub bank: Bank,
	pub revision: i64,
	pub deleted: bool,
	pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
	pub format: u32,
	pub home: String,
	pub epoch: Uuid,
	pub restoring: bool,
	pub units: BTreeMap<Uuid, Fence>,
}

pub fn digest(unit: &Unit) -> Result<String> {
	// Canonical writes truncate timestamps to PostgreSQL's microsecond precision
	// before its input parser can round them. Hash the same canonical values.
	let mut content = unit.content.clone();
	let learned_at = chrono::DateTime::from_timestamp_micros(unit.learned_at.timestamp_micros())
		.ok_or_else(|| Error::Invalid("invalid learning timestamp".into()))?;
	if let Some(range) = content.occurred.as_mut() {
		range.start = chrono::DateTime::from_timestamp_micros(range.start.timestamp_micros())
			.ok_or_else(|| Error::Invalid("invalid occurrence timestamp".into()))?;
		range.end = chrono::DateTime::from_timestamp_micros(range.end.timestamp_micros())
			.ok_or_else(|| Error::Invalid("invalid occurrence timestamp".into()))?;
	}
	Ok(crate::semantic::indexing::content_digest(
		&serde_json::to_string(&(
			&unit.bank,
			unit.revision,
			unit.deleted,
			unit.stale,
			learned_at,
			&content,
		))?,
	))
}
impl Ledger {
	pub fn new(home: String) -> Self {
		Self {
			format: 1,
			home,
			epoch: Uuid::new_v4(),
			restoring: false,
			units: BTreeMap::new(),
		}
	}
	pub fn validate(&self, home: &str) -> Result<()> {
		if self.format != 1
			|| self.home != home
			|| self.epoch.is_nil()
			|| self.units.iter().any(|(id, fence)| {
				id.is_nil()
					|| fence.bank.home != home
					|| fence.bank.validate().is_err()
					|| fence.revision < 1
					|| fence.digest.is_empty()
			}) {
			return Err(Error::Invalid(
				"invalid external memory recovery ledger".into(),
			));
		}
		Ok(())
	}
	/// Persist before the database write. Uncertain or rolled-back writes may
	/// conservatively withhold a restore; a snapshot may never lower this floor.
	pub fn observe(&mut self, unit: &Unit) -> Result<()> {
		if unit.bank.home != self.home || unit.id.is_nil() || unit.revision < 1 {
			return Err(Error::Invalid(
				"invalid memory recovery identity or revision".into(),
			));
		}
		unit.bank.validate()?;
		if let Some(previous) = self.units.get(&unit.id)
			&& (previous.bank != unit.bank
				|| previous.revision > unit.revision
				|| (previous.deleted && !unit.deleted)
				|| (previous.revision == unit.revision
					&& unit.visible()
					&& previous.digest != digest(unit)?))
		{
			return Err(Error::Conflict(
				"external memory revision or deletion fence changed".into(),
			));
		}
		self.units.insert(
			unit.id,
			Fence {
				bank: unit.bank.clone(),
				revision: unit.revision,
				deleted: unit.deleted,
				digest: digest(unit)?,
			},
		);
		Ok(())
	}
	/// Unknown identities are not authorized by an archive, even if its body is
	/// otherwise valid. The independently retained ledger must know every unit.
	pub fn current(&self, unit: &Unit) -> Result<bool> {
		Ok(!unit.deleted && self.matches(unit)?)
	}
	/// Negative identities are checked too, without making them readable facts.
	pub fn matches(&self, unit: &Unit) -> Result<bool> {
		self.validate(&unit.bank.home)?;
		Ok(self.units.get(&unit.id).is_some_and(|fence| {
			fence.bank == unit.bank
				&& fence.revision == unit.revision
				&& fence.deleted == unit.deleted
				&& digest(unit).is_ok_and(|value| value == fence.digest)
		}))
	}
}
