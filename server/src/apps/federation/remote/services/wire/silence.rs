//! A Node that did not answer is not asked again by a list for a while (#770).
use std::{
	collections::HashMap,
	sync::Mutex,
	time::{Duration, Instant},
};

/// How long a list read leaves a silent Node alone.
const MEMORY: Duration = Duration::from_secs(30);

/// Nodes that did not answer, per Tenant and Node, in this process. Lists,
/// searches and the event read consult it so that a poll stops spending up to
/// ten seconds, with a database connection held, on a Node that is down. A
/// single read, a Read Recheck and a Run's inference stream never consult it.
/// It can only turn an answer into "cannot confirm", so it never discloses.
pub(crate) struct SilentNodes {
	until: Mutex<HashMap<(String, String), Instant>>,
	memory: Duration,
}

impl Default for SilentNodes {
	fn default() -> Self {
		Self::remembering(MEMORY)
	}
}

impl SilentNodes {
	pub(crate) fn remembering(memory: Duration) -> Self {
		Self {
			until: Mutex::new(HashMap::new()),
			memory,
		}
	}

	/// `node` did not answer a check of `tenant`.
	pub(crate) fn note(&self, tenant: &str, node: &str) {
		let now = Instant::now();
		let mut until = self.until.lock().expect("silent nodes mutex poisoned");
		until.retain(|_, deadline| *deadline > now);
		until.insert((tenant.to_owned(), node.to_owned()), now + self.memory);
	}

	/// `node` answered `tenant`, with a verdict or a refusal.
	pub(crate) fn clear(&self, tenant: &str, node: &str) {
		self.until
			.lock()
			.expect("silent nodes mutex poisoned")
			.remove(&(tenant.to_owned(), node.to_owned()));
	}

	/// Whether `node` failed to answer `tenant` within the memory.
	pub(crate) fn recent(&self, tenant: &str, node: &str) -> bool {
		self.until
			.lock()
			.expect("silent nodes mutex poisoned")
			.get(&(tenant.to_owned(), node.to_owned()))
			.is_some_and(|deadline| *deadline > Instant::now())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_silent_node_is_remembered_per_tenant_until_it_answers() {
		let silent = SilentNodes::default();
		silent.note("tenant-a", "aidash://other/b");
		assert!(silent.recent("tenant-a", "aidash://other/b"));
		assert!(!silent.recent("tenant-b", "aidash://other/b"));
		silent.clear("tenant-a", "aidash://other/b");
		assert!(!silent.recent("tenant-a", "aidash://other/b"));
	}

	#[test]
	fn a_silent_node_is_forgotten_when_the_memory_runs_out() {
		let silent = SilentNodes::remembering(Duration::ZERO);
		silent.note("tenant-a", "aidash://other/b");
		assert!(!silent.recent("tenant-a", "aidash://other/b"));
	}
}
