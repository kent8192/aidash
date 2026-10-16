//! The Tenants of Runs, Workspaces, tasks and remote grants, remembered once
//! found (#263).
//!
//! Work that names one of these only by id looks its Tenant up before opening
//! the Tenant's scope. A record's Tenant never changes once it is recorded,
//! and Tenant names are never reused, so a found Tenant stays right for the
//! process's lifetime. A record without a Tenant is not remembered: its grant
//! may still be on the way.
use aidash_domain::tenancy::TenantName;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

/// Ids remembered per kind. A full map starts over rather than evicting.
const CAPACITY: usize = 16_384;

#[derive(Default)]
pub(crate) struct Tenancies {
	runs: Ids<TenantName>,
	workspaces: Ids<TenantName>,
	grants: Ids<TenantName>,
	/// A task's Workspace, which never changes either.
	task_workspaces: Ids<Uuid>,
}

impl Tenancies {
	pub(crate) fn run(&self, id: Uuid) -> Option<TenantName> {
		self.runs.get(id)
	}
	pub(crate) fn remember_run(&self, id: Uuid, tenant: TenantName) {
		self.runs.insert(id, tenant);
	}
	pub(crate) fn workspace(&self, id: Uuid) -> Option<TenantName> {
		self.workspaces.get(id)
	}
	pub(crate) fn remember_workspace(&self, id: Uuid, tenant: TenantName) {
		self.workspaces.insert(id, tenant);
	}
	pub(crate) fn grant(&self, id: Uuid) -> Option<TenantName> {
		self.grants.get(id)
	}
	pub(crate) fn remember_grant(&self, id: Uuid, tenant: TenantName) {
		self.grants.insert(id, tenant);
	}
	pub(crate) fn task_workspace(&self, id: Uuid) -> Option<Uuid> {
		self.task_workspaces.get(id)
	}
	pub(crate) fn remember_task_workspace(&self, id: Uuid, workspace: Uuid) {
		self.task_workspaces.insert(id, workspace);
	}
}

struct Ids<V>(Mutex<HashMap<Uuid, V>>);

impl<V> Default for Ids<V> {
	fn default() -> Self {
		Self(Mutex::new(HashMap::new()))
	}
}

impl<V: Clone> Ids<V> {
	fn get(&self, id: Uuid) -> Option<V> {
		self.lock().get(&id).cloned()
	}
	fn insert(&self, id: Uuid, value: V) {
		let mut map = self.lock();
		if map.len() >= CAPACITY {
			map.clear();
		}
		map.insert(id, value);
	}
	/// The map holds plain values, so a panic elsewhere cannot leave it
	/// inconsistent; a poisoned lock is still usable.
	fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, V>> {
		self.0
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn found_tenants_are_remembered_and_a_full_map_starts_over() {
		let tenancies = Tenancies::default();
		let acme = TenantName::new("acme").unwrap();
		let run = Uuid::new_v4();
		assert_eq!(tenancies.run(run), None);
		tenancies.remember_run(run, acme.clone());
		assert_eq!(tenancies.run(run), Some(acme.clone()));
		for _ in 0..CAPACITY {
			tenancies.remember_workspace(Uuid::new_v4(), acme.clone());
		}
		let last = Uuid::new_v4();
		tenancies.remember_workspace(last, acme.clone());
		assert_eq!(tenancies.workspace(last), Some(acme));
		assert_eq!(tenancies.workspaces.lock().len(), 1);
	}
}
