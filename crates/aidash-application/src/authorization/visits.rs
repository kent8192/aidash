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
