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

/// `load_for_update` locks a space until the enclosing transaction ends;
/// `load` is a non-locking read.
/// The scope supplies the authenticated person; all reads are Tenant-scoped.
/// The Run binding resolves delegated Runs on the receiving Node. The returned
/// person is the Run's root person, not the executing Agent.
#[async_trait]
pub trait ScratchScope: Send {
	fn person(&self) -> &str;
	async fn require_create(&mut self, node: &NodeName) -> Result<()>;
	/// Enforce Node access for the authenticated person or the conversation starter.
	async fn require_node_use(&mut self, node: &NodeName, person: &str) -> Result<()>;
	async fn limits(&mut self, node: &NodeName) -> Result<(ScratchCeilings, HostAllowlist)>;
	/// Verify the image and newly added External services for this Tenant and Node.
	/// Existing services are shared with all members and need no individual
	/// Registry-read grant when an unrelated setting changes.
	async fn validate_references(
		&mut self,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> Result<()>;
	/// Recheck runtime-wide image approval without imposing the caller's
	/// Registry permissions on services already shared with the space.
	async fn validate_runtime(&mut self, node: &NodeName, settings: &ScratchSettings)
	-> Result<()>;
	/// Ensure a new space's owner may own it: a person, or the Peer Mapping
	/// credential subject that a delegated Run's space is allocated for.
	async fn validate_owner(&mut self, person: &str) -> Result<()>;
	/// Ensure every member names a person in the scoped Tenant.
	async fn validate_members(
		&mut self,
		members: &std::collections::BTreeSet<String>,
	) -> Result<()>;
	async fn insert(&mut self, space: &ScratchSpace) -> Result<()>;
	async fn load(&mut self, id: ScratchSpaceId) -> Result<ScratchSpace>;
	async fn load_for_update(&mut self, id: ScratchSpaceId) -> Result<ScratchSpace>;
	async fn list(&mut self, node: &NodeName) -> Result<Vec<ScratchSpace>>;
	async fn save(&mut self, space: &ScratchSpace) -> Result<()>;
	async fn delete(&mut self, id: ScratchSpaceId) -> Result<()>;
	/// Record a durable deletion request in this transaction; the adapter must
	/// prevent the old contents from being used after the transaction commits.
	async fn clear_contents(&mut self, id: ScratchSpaceId) -> Result<()>;
	/// Find the creator Run when this workspace belongs to a Subnode.
	async fn subnode_creator_run(&mut self, workspace: uuid::Uuid) -> Result<Option<Run>>;
	async fn run_binding(&mut self, run: &Run) -> Result<(ScratchSpaceId, String)>;
}

fn require_use(space: &ScratchSpace, person: &str, _now: DateTime<Utc>) -> Result<()> {
	if !space.may_use(person) {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub async fn create(scope: &mut dyn ScratchScope, space: &ScratchSpace) -> Result<()> {
	scope.require_create(&space.node).await?;
	create_dedicated(scope, space).await
}

/// A space dedicated to one new Workspace is authorized by creating that
/// Workspace, so it does not need `scratch.create`.
pub async fn create_dedicated(scope: &mut dyn ScratchScope, space: &ScratchSpace) -> Result<()> {
	let person = scope.person().to_owned();
	scope.require_node_use(&space.node, &person).await?;
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
	// The creator must be an enabled owner in this Tenant as well as the
	// authenticated subject; a Run's own Agent cannot become a space owner.
	scope.validate_owner(&person).await?;
	scope.validate_members(&space.members).await?;
	scope.insert(space).await
}

pub async fn list(
	scope: &mut dyn ScratchScope,
	node: &NodeName,
	now: DateTime<Utc>,
) -> Result<Vec<ScratchSpace>> {
	let person = scope.person().to_owned();
	scope.require_node_use(node, &person).await?;
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
	lock: bool,
) -> Result<ScratchSpace> {
	let space = if lock {
		scope.load_for_update(id).await?
	} else {
		scope.load(id).await?
	};
	scope.require_node_use(&space.node, person).await?;
	require_use(&space, person, now)?;
	Ok(space)
}

pub async fn read(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
) -> Result<ScratchSpace> {
	let person = scope.person().to_owned();
	read_for_person(scope, id, &person, now, false).await
}

async fn read_for_update(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
) -> Result<ScratchSpace> {
	let person = scope.person().to_owned();
	read_for_person(scope, id, &person, now, true).await
}

pub async fn update_settings(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
	revision: i64,
	settings: ScratchSettings,
) -> Result<()> {
	let mut space = read_for_update(scope, id, now).await?;
	if space.revision != revision {
		return Err(Error::Conflict(
			"Scratch Space settings changed; reload and retry".into(),
		));
	}
	let (ceilings, tenant_hosts) = scope.limits(&space.node).await?;
	settings.validate(&ceilings, &tenant_hosts)?;
	// Services already shared with the space remain usable by every member.
	// The member changing settings must be allowed to add any new services.
	let mut additions = settings.clone();
	additions.external_services = settings
		.external_services
		.difference(&space.settings.external_services)
		.cloned()
		.collect();
	scope.validate_references(&space.node, &additions).await?;
	let expired = space.is_expired(now);
	if expired {
		space.last_used_at = space.last_used_at.max(now);
	}
	space.settings = settings;
	space.revision = space
		.revision
		.checked_add(1)
		.ok_or_else(|| Error::Conflict("Scratch Space revision exhausted".into()))?;
	scope.save(&space).await?;
	if expired {
		scope.clear_contents(id).await?;
	}
	Ok(())
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
	let mut space = read_for_update(scope, id, now).await?;
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
	let mut space = read_for_update(scope, id, now).await?;
	space.members.remove(member);
	scope.save(&space).await
}

pub async fn clear(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	now: DateTime<Utc>,
) -> Result<()> {
	let person = scope.person().to_owned();
	let mut space = scope.load_for_update(id).await?;
	scope.require_node_use(&space.node, &person).await?;
	if !space.may_modify(&person) {
		return Err(Error::Forbidden);
	}
	space.last_used_at = space.last_used_at.max(now);
	scope.save(&space).await?;
	scope.clear_contents(id).await
}

pub async fn delete(
	scope: &mut dyn ScratchScope,
	id: ScratchSpaceId,
	_now: DateTime<Utc>,
) -> Result<()> {
	let person = scope.person().to_owned();
	let mut space = scope.load_for_update(id).await?;
	scope.require_node_use(&space.node, &person).await?;
	if !space.may_modify(&person) {
		return Err(Error::Forbidden);
	}
	space.tombstone();
	scope.save(&space).await?;
	scope.clear_contents(id).await
}

/// Select the immutable Scratch Space for a new Workspace, creating one
/// dedicated to that Workspace when no existing space was requested.
pub async fn select_for_workspace(
	scope: &mut dyn ScratchScope,
	node: &NodeName,
	name: &str,
	requested: Option<ScratchSpaceId>,
	now: DateTime<Utc>,
) -> Result<ScratchSpaceId> {
	if let Some(id) = requested {
		let mut space = read_for_update(scope, id, now).await?;
		if &space.node != node {
			return Err(Error::Forbidden);
		}
		let (ceilings, tenant_hosts) = scope.limits(node).await?;
		let mut effective = space.settings.clone();
		effective.allowed_hosts = effective.allowed_hosts.intersection(&tenant_hosts);
		effective.validate(&ceilings, &tenant_hosts)?;
		scope.validate_runtime(node, &effective).await?;
		if !space.is_expired(now) {
			space.last_used_at = space.last_used_at.max(now);
			scope.save(&space).await?;
		}
		return Ok(id);
	}
	let (_, tenant_hosts) = scope.limits(node).await?;

	let space = ScratchSpace {
		id: uuid::Uuid::new_v4(),
		node: node.clone(),
		name: name.to_owned(),
		creator: scope.person().to_owned(),
		members: Default::default(),
		state: ScratchState::Active,
		revision: 0,
		wipe_generation: 0,
		settings: ScratchSettings::for_tenant(&tenant_hosts),
		created_at: now,
		last_used_at: now,
	};
	create_dedicated(scope, &space).await?;
	Ok(space.id)
}

pub async fn scratch_space_for_run(
	scope: &mut dyn ScratchScope,
	run: &Run,
	now: DateTime<Utc>,
) -> Result<ScratchSpaceId> {
	let creator = scope.subnode_creator_run(run.workspace_id).await?;
	let binding_run = creator.as_ref().unwrap_or(run);
	let (id, starter) = scope.run_binding(binding_run).await?;
	// Run admission already checks the executing Node's authority; here the
	// starter only has to still be allowed to use the space.
	let mut space = scope.load_for_update(id).await?;
	require_use(&space, &starter, now)?;
	if space.node != run.node {
		return Err(Error::Forbidden);
	}
	// Operator decisions, Tenant hosts, and image approval can change after a
	// space was created. Recheck them without narrowing shared services to the
	// executing Agent's individual Registry permissions.
	let (ceilings, tenant_hosts) = scope.limits(&space.node).await?;
	let mut effective = space.settings.clone();
	effective.allowed_hosts = effective.allowed_hosts.intersection(&tenant_hosts);
	effective.validate(&ceilings, &tenant_hosts)?;
	scope.validate_runtime(&space.node, &effective).await?;
	let expired = space.is_expired(now);
	space.last_used_at = space.last_used_at.max(now);
	scope.save(&space).await?;
	if expired {
		scope.clear_contents(id).await?;
	}
	Ok(id)
}

#[cfg(test)]
mod tests;
