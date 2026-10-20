//! Owned recursion guards release their visit on success, failure, panic or cancellation.
use std::{
	collections::{BTreeMap, btree_map::Entry},
	sync::{Arc, Mutex},
};
use uuid::Uuid;
pub type ReadVisitKey = (String, Uuid, String);
#[derive(Default)]
pub struct ReadVisits {
	active: Arc<Mutex<BTreeMap<ReadVisitKey, Arc<()>>>>,
}
pub struct ReadVisit {
	active: Arc<Mutex<BTreeMap<ReadVisitKey, Arc<()>>>>,
	key: ReadVisitKey,
	identity: Arc<()>,
}
impl ReadVisits {
	pub fn enter(&self, key: ReadVisitKey) -> Option<ReadVisit> {
		let mut active = self.active.lock().expect("read visits mutex poisoned");
		match active.entry(key.clone()) {
			Entry::Occupied(_) => None,
			Entry::Vacant(entry) => {
				let identity = Arc::new(());
				entry.insert(identity.clone());
				Some(ReadVisit {
					active: self.active.clone(),
					key,
					identity,
				})
			}
		}
	}
	pub fn clear(&self) {
		self.active
			.lock()
			.expect("read visits mutex poisoned")
			.clear();
	}
	/// No check of any kind is in progress, so a verdict reached now rests on
	/// no unresolved ancestor.
	pub fn is_idle(&self) -> bool {
		self.active
			.lock()
			.expect("read visits mutex poisoned")
			.is_empty()
	}
}

/// Read-provenance verdicts within one authority, so each Run's reads are
/// walked once rather than once per path that reaches it.
///
/// A cycle answers `true` for a Run still being checked, so a `true` reached
/// inside a check may rest on an ancestor that later fails. It stays
/// provisional until the outermost check ends `true`, and is dropped
/// otherwise. A `false` comes from a policy that denied a read and is final.
#[derive(Default)]
pub struct ReadVerdicts {
	settled: BTreeMap<ReadVisitKey, bool>,
	provisional: std::collections::BTreeSet<ReadVisitKey>,
}
impl ReadVerdicts {
	pub fn get(&self, key: &ReadVisitKey) -> Option<bool> {
		self.settled
			.get(key)
			.copied()
			.or_else(|| self.provisional.contains(key).then_some(true))
	}
	/// Records a completed check. `outermost` says no other check was in
	/// progress when it began.
	pub fn record(&mut self, key: ReadVisitKey, verdict: bool, outermost: bool) {
		if !verdict {
			self.settled.insert(key, false);
		} else {
			self.provisional.insert(key);
		}
		if outermost {
			let provisional = std::mem::take(&mut self.provisional);
			if verdict {
				self.settled
					.extend(provisional.into_iter().map(|key| (key, true)));
			}
		}
	}
	/// An outermost check failed without a verdict; nothing it reached is settled.
	pub fn abandon(&mut self) {
		self.provisional.clear();
	}
	pub fn clear(&mut self) {
		self.settled.clear();
		self.provisional.clear();
	}
}
impl Drop for ReadVisit {
	fn drop(&mut self) {
		let mut active = self.active.lock().expect("read visits mutex poisoned");
		// A refreshed authority may have cleared and re-entered this key. An old guard cannot remove it.
		if active
			.get(&self.key)
			.is_some_and(|identity| Arc::ptr_eq(identity, &self.identity))
		{
			active.remove(&self.key);
		}
	}
}
#[cfg(test)]
mod tests;
