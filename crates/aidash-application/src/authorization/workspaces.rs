//! Ownership, disclosure and interaction share the caller's exact authority snapshot.
use crate::{
	Error, Result,
	ports::authorization::workspaces::{
		RunInteractionScope, WorkspaceAuthorityScope, WorkspaceResourceScope,
	},
};
use aidash_domain::{
	Run,
	policy::{Evaluation, Resource},
};
use serde_json::json;
use uuid::Uuid;
/// An inherited lease reaches its own Workspace and, for reading only, the
/// Subnode Workspaces its Run created (named by the lease context).
pub fn workspace_reachable(context: &serde_json::Value, id: Uuid) -> bool {
	let id = json!(id.to_string());
	context["workspace_id"] == id || subnode_read_only(context, &id)
}
/// Whether `id` is reachable only through the creator's read exception.
pub fn subnode_read_workspace(context: &serde_json::Value, id: Uuid) -> bool {
	let id = json!(id.to_string());
	context["workspace_id"] != id && subnode_read_only(context, &id)
}
fn subnode_read_only(context: &serde_json::Value, id: &serde_json::Value) -> bool {
	context["subnode_read_workspaces"]
		.as_array()
		.is_some_and(|workspaces| workspaces.contains(id))
}
pub async fn resource(scope: &mut dyn WorkspaceResourceScope, id: Uuid) -> Result<Resource> {
	if scope.inherited() && !workspace_reachable(scope.context(), id) {
		return Err(Error::Forbidden);
	}
	let lock = !scope.inherited();
	let owner = scope.owner(id, lock).await?.ok_or(Error::Forbidden)?;
	Ok(scope.resource(
		"workspace",
		&id.to_string(),
		json!({"owner":owner,"workspace_id":id}),
	))
}
pub async fn decision(
	scope: &mut dyn WorkspaceAuthorityScope,
	id: Uuid,
	action: &str,
	owner: Option<&str>,
) -> Result<bool> {
	let (tenant, subject) = scope.identity();
	let input = Evaluation {
		subject: subject.into(),
		action: action.into(),
		resource: Resource {
			tenant: tenant.into(),
			kind: "workspace".into(),
			id: id.to_string(),
			attributes: owner
				.map(|owner| json!({"owner":owner,"workspace_id":id}))
				.unwrap_or_else(|| json!({})),
		},
		environment: scope.environment().clone(),
	};
	let mut decision = scope.snapshot().bundle.evaluate(&input);
	decision.revision = scope.snapshot().revision;
	if owner.is_none() {
		decision.allowed = false;
		decision.reason = "resource_unavailable".into();
	}
	let allowed = decision.allowed;
	scope.record(&[(input, decision)]).await?;
	Ok(allowed)
}
pub async fn allowed(
	scope: &mut dyn WorkspaceAuthorityScope,
	id: Uuid,
	action: &str,
) -> Result<bool> {
	let owner = scope.locked_owner(id).await?;
	decision(scope, id, action, owner.as_deref()).await
}
pub async fn require(
	scope: &mut dyn WorkspaceAuthorityScope,
	id: Uuid,
	action: &str,
) -> Result<()> {
	if allowed(scope, id, action).await? {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
pub async fn visible(scope: &mut dyn WorkspaceAuthorityScope, action: &str) -> Result<Vec<Uuid>> {
	let mut result = vec![];
	for (id, owner) in scope.all_owners().await? {
		if decision(scope, id, action, Some(&owner)).await? {
			result.push(id);
		}
	}
	Ok(result)
}
pub async fn event_workspaces(
	scope: &mut dyn WorkspaceAuthorityScope,
	selected: Option<Uuid>,
) -> Result<Vec<Uuid>> {
	let rows = scope.event_owners(selected).await?;
	if rows.is_empty()
		&& let Some(id) = selected
	{
		decision(scope, id, "workspace.read", None).await?;
	}
	let mut visible = vec![];
	for (id, owner) in rows {
		if decision(scope, id, "workspace.read", Some(&owner)).await?
			&& decision(scope, id, "workspace.events", Some(&owner)).await?
		{
			visible.push(id);
		}
	}
	Ok(visible)
}
pub async fn run_for_interaction(scope: &mut dyn RunInteractionScope, id: Uuid) -> Result<Run> {
	let run = scope.run(id).await?.ok_or(Error::Forbidden)?;
	let workspace = resource(scope, run.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	scope.require(&workspace, "workspace.read").await?;
	if !scope.run_visible(&(&run).into()).await? {
		return Err(Error::Forbidden);
	}
	Ok(run)
}
#[cfg(test)]
mod tests;
