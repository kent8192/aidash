//! Receiver evidence is short-lived and belongs to one version of a Home grant.
use crate::ports::federation::dependencies::{Check, DependencyTransport};
use aidash_domain::{
	federation::{
		dependencies::{Checked, Input, Reference},
		execution::{Inspection, home::Grant},
		route::Route,
	},
	registry::rules::digest,
};
use async_trait::async_trait;
use serde_json::json;
use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::time::Instant;

const TTL: Duration = Duration::from_secs(30);
const CACHE_CAPACITY: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Revision(String);

impl Revision {
	pub(super) fn new(grant: &Grant, task_revision: i64, home: &str) -> Self {
		// Grants have no separate revision counter. Cover their saved authority,
		// including the pinned receiver Agent version and definition.
		Self(digest(&json!([
			grant.id,
			grant.task_id,
			grant.task_revision,
			task_revision,
			grant.thread_id,
			home,
			grant.node_id,
			grant.tenant,
			grant.credential_id,
			grant.root_subject,
			grant.subject_chain,
			grant.inspection,
			grant.expires_at,
			grant.revoked,
			grant.semantic,
		])))
	}
}

#[derive(Clone)]
pub(super) struct Observed<T> {
	pub(super) value: T,
	at: Instant,
}

impl<T> Observed<T> {
	pub(super) fn new(value: T) -> Self {
		Self {
			value,
			at: Instant::now(),
		}
	}

	pub(super) fn current(&self) -> bool {
		self.at.elapsed() <= TTL
	}
}

struct Cache<K, V>(Mutex<VecDeque<(K, Observed<V>)>>);

impl<K, V> Default for Cache<K, V> {
	fn default() -> Self {
		Self(Mutex::new(VecDeque::new()))
	}
}

impl<K: PartialEq, V: Clone> Cache<K, V> {
	fn get(&self, key: &K) -> Option<Observed<V>> {
		self.0
			.lock()
			.expect("receiver evidence cache")
			.iter()
			.find(|(found, value)| found == key && value.current())
			.map(|(_, value)| value.clone())
	}

	fn put(&self, key: K, value: Observed<V>) {
		let mut entries = self.0.lock().expect("receiver evidence cache");
		entries.retain(|(found, entry)| *found != key && entry.current());
		if entries.len() >= CACHE_CAPACITY {
			entries.pop_front();
		}
		entries.push_back((key, value));
	}
}

/// Bounded process-local replies. Keys include the grant revision so an older
/// request finishing after a revision change cannot supply evidence for it.
#[derive(Default)]
pub struct GrantPeerCache {
	inspections: Cache<Revision, Inspection>,
	dependencies: Cache<(Revision, Route, Reference), Option<Checked>>,
}

impl GrantPeerCache {
	pub(super) fn inspection(&self, revision: &Revision) -> Option<Observed<Inspection>> {
		self.inspections.get(revision)
	}

	pub(super) fn remember_inspection(&self, revision: Revision, inspection: Observed<Inspection>) {
		self.inspections.put(revision, inspection);
	}
}

/// The grant lease only reads peer evidence. A miss records the exact request
/// so the caller can release the lease before asking the peer.
#[derive(Clone)]
pub struct DependencyChecks {
	cache: Arc<GrantPeerCache>,
	revision: Revision,
	missing: Arc<Mutex<Option<(Route, Input)>>>,
}

impl DependencyChecks {
	pub(super) fn new(cache: Arc<GrantPeerCache>, revision: Revision) -> Self {
		Self {
			cache,
			revision,
			missing: Default::default(),
		}
	}

	pub(super) fn take_missing(&self) -> Option<(Route, Input)> {
		self.missing
			.lock()
			.expect("grant dependency request")
			.take()
	}

	pub(super) fn remember(&self, route: Route, input: Input, check: Check) -> bool {
		let reply = match check {
			Check::Confirmed(checked) => Some(checked),
			Check::Refused => None,
			Check::Unavailable | Check::ResponseTooLarge(_) | Check::NotAsked => return false,
		};
		self.cache.dependencies.put(
			(self.revision.clone(), route, input.reference),
			Observed::new(reply),
		);
		true
	}
}

#[async_trait]
impl DependencyTransport for DependencyChecks {
	async fn check(&self, route: &Route, input: &Input, _: Duration) -> Check {
		let key = (
			self.revision.clone(),
			route.clone(),
			input.reference.clone(),
		);
		match self.cache.dependencies.get(&key) {
			Some(Observed {
				value: Some(checked),
				..
			}) => Check::Confirmed(checked),
			Some(Observed { value: None, .. }) => Check::Refused,
			None => {
				*self.missing.lock().expect("grant dependency request") =
					Some((route.clone(), input.clone()));
				Check::NotAsked
			}
		}
	}
}

#[cfg(test)]
mod tests;
