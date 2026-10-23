//! Ordered non-body history for independently durable memory floors.
use super::{Fence, Ledger, Unit};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Head {
	pub sequence: u64,
	pub digest: String,
	pub generation: u64,
	pub recovering: bool,
}

/// A body-free observation. Visibility distinguishes allowed hidden-body cleanup
/// from an impermissible visible rewrite at the same canonical revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Floor {
	pub id: Uuid,
	pub fence: Fence,
	pub visible: bool,
}
impl Floor {
	pub fn from_unit(unit: &Unit) -> Result<Self> {
		Ok(Self {
			id: unit.id,
			fence: Fence {
				bank: unit.bank.clone(),
				revision: unit.revision,
				deleted: unit.deleted,
				digest: super::digest(unit)?,
			},
			visible: unit.visible(),
		})
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
	Floors { units: Vec<Floor>, recovering: bool },
	Barrier,
	Resume { evidence: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
	pub format: u32,
	pub home: String,
	pub epoch: Uuid,
	pub sequence: u64,
	pub predecessor: String,
	pub operation: Uuid,
	pub generation: u64,
	pub change: Change,
}
impl Record {
	pub fn digest(&self) -> Result<String> {
		Ok(crate::semantic::indexing::content_digest(
			&serde_json::to_string(self)?,
		))
	}
}

#[derive(Debug, Clone)]
pub struct History {
	pub ledger: Ledger,
	pub head: Head,
}
impl History {
	pub fn new(ledger: Ledger, seed_digest: String) -> Result<Self> {
		ledger.validate(&ledger.home)?;
		if seed_digest.len() != 64 || !seed_digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
			return Err(Error::Invalid("invalid memory history seed digest".into()));
		}
		Ok(Self {
			ledger,
			head: Head {
				sequence: 0,
				digest: seed_digest,
				generation: 0,
				recovering: false,
			},
		})
	}
	pub fn next(&self, operation: Uuid, change: Change) -> Result<Record> {
		let increment = matches!(change, Change::Barrier | Change::Resume { .. });
		Ok(Record {
			format: 1,
			home: self.ledger.home.clone(),
			epoch: self.ledger.epoch,
			sequence: self
				.head
				.sequence
				.checked_add(1)
				.ok_or_else(|| Error::Invalid("memory history sequence exhausted".into()))?,
			predecessor: self.head.digest.clone(),
			operation,
			generation: self
				.head
				.generation
				.checked_add(u64::from(increment))
				.ok_or_else(|| Error::Invalid("memory history generation exhausted".into()))?,
			change,
		})
	}
	/// Validate a complete record before changing any floor or the applied head.
	pub fn apply(&mut self, record: &Record) -> Result<()> {
		let expected = self.next(record.operation, record.change.clone())?;
		if record.format != 1
			|| record.home != self.ledger.home
			|| record.epoch != self.ledger.epoch
			|| record.operation.is_nil()
			|| record.sequence != expected.sequence
			|| record.predecessor != self.head.digest
			|| record.generation != expected.generation
		{
			return Err(Error::Conflict(
				"memory history identity, predecessor or generation changed".into(),
			));
		}
		let mut ledger = self.ledger.clone();
		let recovering = match &record.change {
			Change::Floors { units, recovering } => {
				if *recovering != self.head.recovering || units.is_empty() {
					return Err(Error::Conflict(
						"memory publication crossed a recovery barrier".into(),
					));
				}
				let mut seen = BTreeSet::new();
				for unit in units {
					if unit.id.is_nil()
						|| !seen.insert(unit.id)
						|| unit.fence.bank.home != ledger.home
						|| unit.fence.revision < 1
						|| unit.fence.digest.is_empty()
						|| (unit.visible && unit.fence.deleted)
					{
						return Err(Error::Invalid(
							"invalid or repeated memory history floor".into(),
						));
					}
					unit.fence.bank.validate()?;
					if let Some(previous) = ledger.units.get(&unit.id)
						&& (previous.bank != unit.fence.bank
							|| previous.revision > unit.fence.revision
							|| (previous.deleted && !unit.fence.deleted)
							|| (previous.revision == unit.fence.revision
								&& unit.visible && previous.digest != unit.fence.digest))
					{
						return Err(Error::Conflict(
							"memory history would rewind a revision, deletion or body fence".into(),
						));
					}
					ledger.units.insert(unit.id, unit.fence.clone());
				}
				*recovering
			}
			Change::Barrier => true,
			Change::Resume { evidence } => {
				if !self.head.recovering || evidence.trim().is_empty() || evidence.len() > 256 {
					return Err(Error::Conflict(
						"memory recovery resume requires complete reconciliation evidence".into(),
					));
				}
				false
			}
		};
		ledger.validate(&ledger.home)?;
		self.head = Head {
			sequence: record.sequence,
			digest: record.digest()?,
			generation: record.generation,
			recovering,
		};
		self.ledger = ledger;
		Ok(())
	}
}

#[cfg(test)]
mod tests;
