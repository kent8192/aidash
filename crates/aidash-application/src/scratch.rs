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
/// The Run binding must resolve a Subnode to its creator's space and a delegated
/// Run to a binding on the receiving Node. The returned person is the person
/// who started the conversation, not the Agent executing the Run.
#[async_trait]
pub trait ScratchScope: Send {
	async fn require_create(&mut self, node: &NodeName) -> Result<()>;
	async fn limits(&mut self, node: &NodeName) -> Result<(ScratchCeilings, HostAllowlist)>;
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

pub async fn create(
	scope: &mut dyn ScratchScope,
	space: &ScratchSpace,
	person: &str,
) -> Result<()> {
	scope.require_create(&space.node).await?;
	let (ceilings, tenant_hosts) = scope.limits(&space.node).await?;
	space.settings.validate(&ceilings, &tenant_hosts)?;
	if space.name.trim().is_empty()
		|| person.trim().is_empty()
		|| space.creator != person
		|| space.state != ScratchState::Active
	{
		return Err(Error::Invalid(
			"Scratch Space name and creator are required".into(),
		));
	}
	scope.insert(space).await
}

pub async fn list(
	scope: &mut dyn ScratchScope,
	node: &NodeName,
	person: &str,
	now: DateTime<Utc>,
) -> Result<Vec<ScratchSpace>> {
	Ok(scope
		.list(node)
		.await?
		.into_iter()
		.filter(|space| require_use(space, person, now).is_ok())
		.collect())
}

pub async fn read(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	person: &str,
	now: DateTime<Utc>,
) -> Result<ScratchSpace> {
	let space = scope.load_for_update(id).await?;
	require_use(&space, person, now)?;
	Ok(space)
}

pub async fn update_settings(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	person: &str,
	now: DateTime<Utc>,
	settings: ScratchSettings,
) -> Result<()> {
	let mut space = read(scope, id, person, now).await?;
	let (ceilings, tenant_hosts) = scope.limits(&space.node).await?;
	settings.validate(&ceilings, &tenant_hosts)?;
	space.settings = settings;
	scope.save(&space).await
}

pub async fn add_member(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	person: &str,
	now: DateTime<Utc>,
	member: &str,
) -> Result<()> {
	if member.trim().is_empty() {
		return Err(Error::Invalid("member is required".into()));
	}
	let mut space = read(scope, id, person, now).await?;
	space.members.insert(member.to_owned());
	scope.save(&space).await
}

pub async fn remove_member(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	person: &str,
	now: DateTime<Utc>,
	member: &str,
) -> Result<()> {
	let mut space = read(scope, id, person, now).await?;
	space.members.remove(member);
	scope.save(&space).await
}

pub async fn clear(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	person: &str,
	now: DateTime<Utc>,
) -> Result<()> {
	let mut space = read(scope, id, person, now).await?;
	scope.clear_contents(id).await?;
	space.clear();
	scope.save(&space).await
}

pub async fn delete(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	person: &str,
	now: DateTime<Utc>,
) -> Result<()> {
	read(scope, id, person, now).await?;
	scope.clear_contents(id).await?;
	scope.delete(id).await
}

pub async fn scratch_space_for_run(
	scope: &mut dyn ScratchScope,
	run: &Run,
	now: DateTime<Utc>,
) -> Result<ScratchSpaceId> {
	let (id, starter) = scope.run_binding(run).await?;
	let mut space = read(scope, id, &starter, now).await?;
	if space.node != run.node {
		return Err(Error::Forbidden);
	}
	space.last_used_at = space.last_used_at.max(now);
	scope.save(&space).await?;
	Ok(id)
}

#[cfg(test)]
mod tests;
