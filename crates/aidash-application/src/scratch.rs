//! Authorized Scratch Space operations. The scope owns the transaction.
use aidash_domain::{
	Run,
	nodes::NodeName,
	scratch::{
		HostAllowlist, ScratchCeilings, ScratchSettings, ScratchSpace, ScratchSpaceId, ScratchState,
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::{Error, Result};

/// Implementations lock a loaded space until the enclosing transaction ends.
/// The scope supplies the authenticated person; all reads are Tenant-scoped.
/// The Run binding must resolve a Subnode to its creator's space and a delegated
/// Run to a binding on the receiving Node. The returned person is the person
/// who started the conversation, not the Agent executing the Run.
#[async_trait]
pub trait ScratchScope: Send {
	fn person(&self) -> &str;
	async fn require_create(&mut self, node: &NodeName) -> Result<()>;
	async fn limits(&mut self, node: &NodeName) -> Result<(ScratchCeilings, HostAllowlist)>;
	/// Verify referenced images and External services are approved for this Tenant and Node.
	async fn validate_references(
		&mut self,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> Result<()>;
	/// Ensure every member names a person in the scoped Tenant.
	async fn validate_members(
		&mut self,
		members: &std::collections::BTreeSet<String>,
	) -> Result<()>;
	async fn insert(&mut self, space: &ScratchSpace) -> Result<()>;
	async fn load_for_update(&mut self, id: ScratchSpaceId) -> Result<ScratchSpace>;
	async fn list(&mut self, node: &NodeName) -> Result<Vec<ScratchSpace>>;
	async fn save(&mut self, space: &ScratchSpace) -> Result<()>;
	async fn delete(&mut self, id: ScratchSpaceId) -> Result<()>;
	/// Record a durable deletion request in this transaction; the adapter must
	/// prevent the old contents from being used after the transaction commits.
	async fn clear_contents(&mut self, id: ScratchSpaceId) -> Result<()>;
	async fn run_binding(&mut self, run: &Run) -> Result<(ScratchSpaceId, String)>;
}

fn require_use(space: &ScratchSpace, person: &str, now: DateTime<Utc>) -> Result<()> {
	if !space.may_use(person) || space.is_expired(now) {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub async fn create(scope: &mut dyn ScratchScope, space: &ScratchSpace) -> Result<()> {
	let person = scope.person().to_owned();
	scope.require_create(&space.node).await?;
	let (ceilings, tenant_hosts) = scope.limits(&space.node).await?;
	space.settings.validate(&ceilings, &tenant_hosts)?;
	scope
		.validate_references(&space.node, &space.settings)
		.await?;
	if space.name.trim().is_empty()
		|| person.trim().is_empty()
		|| space.creator != person
		|| space.state != ScratchState::Active
	{
		return Err(Error::Invalid(
			"Scratch Space name and creator are required".into(),
		));
	}
	scope.validate_members(&space.members).await?;
	scope.insert(space).await
}

pub async fn list(
	scope: &mut dyn ScratchScope,
	node: &NodeName,
	now: DateTime<Utc>,
) -> Result<Vec<ScratchSpace>> {
	let person = scope.person().to_owned();
	Ok(scope
		.list(node)
		.await?
		.into_iter()
		.filter(|space| require_use(space, &person, now).is_ok())
		.collect())
}

async fn read_for_person(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	person: &str,
	now: DateTime<Utc>,
) -> Result<ScratchSpace> {
	let space = scope.load_for_update(id).await?;
	require_use(&space, person, now)?;
	Ok(space)
}

pub async fn read(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
) -> Result<ScratchSpace> {
	let person = scope.person().to_owned();
	read_for_person(scope, id, &person, now).await
}

pub async fn update_settings(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
	settings: ScratchSettings,
) -> Result<()> {
	let mut space = read(scope, id, now).await?;
	let (ceilings, tenant_hosts) = scope.limits(&space.node).await?;
	settings.validate(&ceilings, &tenant_hosts)?;
	scope.validate_references(&space.node, &settings).await?;
	space.settings = settings;
	scope.save(&space).await
}

pub async fn add_member(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
	member: &str,
) -> Result<()> {
	if member.trim().is_empty() {
		return Err(Error::Invalid("member is required".into()));
	}
	let mut space = read(scope, id, now).await?;
	space.members.insert(member.to_owned());
	scope.validate_members(&space.members).await?;
	scope.save(&space).await
}

pub async fn remove_member(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
	member: &str,
) -> Result<()> {
	let mut space = read(scope, id, now).await?;
	space.members.remove(member);
	scope.save(&space).await
}

pub async fn clear(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
) -> Result<()> {
	let mut space = read(scope, id, now).await?;
	scope.clear_contents(id).await?;
	space.clear();
	scope.save(&space).await
}

pub async fn delete(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
) -> Result<()> {
	read(scope, id, now).await?;
	scope.clear_contents(id).await?;
	scope.delete(id).await
}

pub async fn scratch_space_for_run(
	scope: &mut dyn ScratchScope,
	run: &Run,
	now: DateTime<Utc>,
) -> Result<ScratchSpaceId> {
	let (id, starter) = scope.run_binding(run).await?;
	let mut space = read_for_person(scope, id, &starter, now).await?;
	if space.node != run.node {
		return Err(Error::Forbidden);
	}
	space.last_used_at = space.last_used_at.max(now);
	scope.save(&space).await?;
	Ok(id)
}

#[cfg(test)]
mod tests;
