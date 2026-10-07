//! Inspection evaluates each pinned component in the same worker execution context.
use crate::{Error, Result, ports::registry::workbench::permissions::PermissionRepository};
use aidash_domain::{
	identity::Principal,
	policy::{Evaluation, Resource},
	registry::{
		EntityRef,
		workbench::permissions::{PermissionContext, PermissionInput, PermissionRow},
	},
};
use chrono::Utc;
use serde_json::json;
pub async fn inspect(
	repository: &dyn PermissionRepository,
	reference: EntityRef,
	input: PermissionInput,
) -> Result<PermissionContext> {
	if let Principal::Subject { tenant, subject } = repository.principal()
		&& (input.tenant != tenant || input.subject != subject)
	{
		return Err(Error::Forbidden);
	}
	let mut scope = repository.begin().await?;
	scope.require_inspection(&reference).await?;
	let entry = scope.effective(&reference).await?;
	if entry.kind != "agent" {
		return Err(Error::NotFound("agent version".into()));
	}
	let snapshot = scope.bindings(&entry).await?;
	// Receiver definitions retain their Node qualifier. Home can preview its own
	// dispatch permission, while the receiver evaluates its resource authority.
	let components = snapshot
		.definitions
		.iter()
		.filter(|d| d.identity.registry_node == repository.node_id())
		.map(|d| {
			(
				d.identity.local(),
				crate::registry::bindings::component_action(&d.definition.kind),
			)
		});
	let mut rows = Vec::new();
	let mut policy_revision = 0;
	for (reference, action) in components {
		let dependency = scope.effective(&reference).await?;
		let catalog_enabled = scope.catalog_enabled(&input.tenant, &reference).await?;
		let mut evaluation = Evaluation {
			subject: input.subject.clone(),
			action: action.into(),
			resource: Resource {
				tenant: input.tenant.clone(),
				kind: dependency.kind.clone(),
				id: if dependency.kind == "tool" {
					aidash_domain::registry::bindings::QualifiedRef {
						registry_node: repository.node_id().into(),
						id: reference.id.clone(),
						version: reference.version.clone(),
					}
					.resource_id()
				} else {
					reference.id.clone()
				},
				attributes: json!({"version":reference.version,"capabilities":dependency.capabilities,"tags":dependency.tags,"languages":dependency.languages,"config":dependency.config}),
			},
			environment: json!({"workspace_id":input.workspace_id,"node_id":repository.node_id(),"transport":"worker"}),
		};
		let decision = scope.evaluate(&input.tenant, &evaluation).await?;
		let registry_read_allowed = if action == "agent.execute" {
			None
		} else {
			evaluation.action = "registry.read".into();
			evaluation.resource.id = reference.id.clone();
			Some(scope.evaluate(&input.tenant, &evaluation).await?.allowed)
		};
		policy_revision = decision.revision;
		rows.push(PermissionRow {
			reference,
			kind: dependency.kind,
			action: action.into(),
			catalog_enabled: catalog_enabled == Some(true),
			policy_allowed: decision.allowed,
			registry_read_allowed,
			effective_for_component: catalog_enabled == Some(true)
				&& decision.allowed
				&& registry_read_allowed.unwrap_or(true),
		});
	}
	let workspace_read = if let Some(workspace_id) = input.workspace_id {
		if let Some(owner) = scope.workspace_owner(workspace_id, &input.tenant).await? {
			let decision = scope.evaluate(&input.tenant, &Evaluation {
                subject: input.subject.clone(),
                action: "workspace.read".into(),
                resource: Resource {
                    tenant: input.tenant.clone(),
                    kind: "workspace".into(),
                    id: workspace_id.to_string(),
                    attributes: json!({"owner":owner,"workspace_id":workspace_id}),
                },
                environment: json!({"workspace_id":workspace_id,"node_id":repository.node_id(),"transport":"worker"}),
            }).await?;
			policy_revision = decision.revision;
			Some(decision.allowed)
		} else {
			Some(false)
		}
	} else {
		None
	};
	scope.commit().await?;
	Ok(PermissionContext {
        tenant: input.tenant,
        subject: input.subject,
        workspace_id: input.workspace_id,
        policy_revision,
        observed_at: Utc::now(),
        requested_capabilities: entry.capabilities,
        rows,
        workspace_read,
        note:"Component-level decisions use this Node's worker execution context and include required Registry reads; foreign receivers evaluate their own dependencies. Task and execution admission require further checks. No universal permission or Trust assessment is implied.".into(),
    })
}
#[cfg(test)]
mod tests;
