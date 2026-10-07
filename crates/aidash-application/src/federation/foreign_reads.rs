//! Foreign Run disclosure rechecks viewer, mapped source and exact definitions.
use crate::{Error, Result, ports::federation::foreign_reads::ForeignRunReadScope};
use aidash_domain::{
	RunControl, RunMetadata, policy::SubjectKind, qualified_agent, registry::rules::digest,
};
use serde_json::json;

pub async fn visible(scope: &mut dyn ForeignRunReadScope, run: &RunMetadata) -> Result<bool> {
	let Some(record) = scope.admission(run).await? else {
		return Ok(false);
	};
	let d = record.description;
	if d.source_node != run.home_node
		|| d.target_node != scope.node()
		|| d.task.id != run.task_id
		|| d.task.workspace_id != run.workspace_id
		|| d.inspection.agent.id != run.agent_id
		|| d.inspection.agent.version != run.agent_version
		|| d.expires_at <= scope.now()
	{
		return Ok(false);
	}
	let workspace = scope.resource(
		"workspace",
		&format!("{}/workspaces/{}", run.home_node, run.workspace_id),
		json!({}),
	);
	let task = scope.resource(
		"task",
		&format!("{}/tasks/{}", run.home_node, run.task_id),
		json!({"created_by":d.task.created_by,"requirements":d.task.requirements}),
	);
	let memory=scope.resource("memory",&run.agent_id,json!({"created_by":qualified_agent(scope.node(),&run.agent_id,&run.agent_version),"version":run.agent_version}));
	if !scope.decide(&workspace, "workspace.read").await?
		|| !scope.decide(&task, "task.read").await?
		|| !scope
			.decide(
				&scope.resource("run", &run.id.to_string(), json!({})),
				"run.read",
			)
			.await?
		|| !scope.decide(&memory, "memory.read").await?
	{
		return Ok(false);
	}
	if scope.mapped_credential(run, &d).await? != Some(record.credential_id) {
		return Ok(false);
	}
	let Some(root) = record.subjects.first() else {
		return Ok(false);
	};
	let mut snapshot = match scope.source_snapshot(record.credential_id, root).await {
		Ok(snapshot) => snapshot,
		Err(Error::Unauthorized | Error::Forbidden) => return Ok(false),
		Err(error) => return Err(error),
	};
	let terminal_statuses: &[&str] = match run.phase.as_str() {
		"COMPLETED" => &["COMPLETED"],
		"FAILED" => &["FAILED"],
		"CANCELLED" => &["STOPPED", "EXPIRED"],
		_ if run.control == RunControl::Cancelled => &["STOPPED", "EXPIRED"],
		_ => &[],
	};
	let historical = if !terminal_statuses.is_empty() && d.inspection.generation.is_some() {
		scope
			.retired_definition(run, &d, record.credential_id, terminal_statuses)
			.await?
	} else {
		None
	};
	if historical.is_some() {
		let executor = qualified_agent(scope.node(), &run.agent_id, &run.agent_version);
		let Some(subject) = snapshot.bundle.subjects.get_mut(&executor) else {
			return Ok(false);
		};
		if subject.kind != SubjectKind::Agent || record.subjects.last() != Some(&executor) {
			return Ok(false);
		}
		// The exact retirement fence allows historical disclosure while current
		// roles, denies and every delegator continue to govern this read only.
		subject.enabled = true;
	}
	let mut source = scope.source_authority(snapshot, record.subjects);
	if !source.decide(&workspace, "workspace.read").await?
		|| !source.decide(&task, "task.read").await?
		|| !source.decide(&task, "task.execute").await?
		|| (!d.semantic.disabled() && !source.decide(&workspace, "semantic.use").await?)
	{
		return Ok(false);
	}
	for definition in &d.inspection.definitions {
		let entry = if let Some(entry) = historical.as_ref().filter(|entry| {
			entry.id == definition.entry.id && entry.version == definition.entry.version
		}) {
			if !source
				.decide(&source.catalog_resource(entry), "registry.read")
				.await?
			{
				return Ok(false);
			}
			entry.clone()
		} else {
			match source
				.catalog_entry(&definition.entry, "registry.read")
				.await
			{
				Ok(entry) => entry,
				Err(Error::Forbidden | Error::NotFound(_)) => return Ok(false),
				Err(error) => return Err(error),
			}
		};
		let action = match entry.kind.as_str() {
			"agent" => "agent.execute",
			"model" => "model.infer",
			"tool" => "tool.invoke",
			"skill" => "skill.use",
			"cluster" => "cluster.execute",
			"compactor" => "compaction.invoke",
			"bundle" | "memory" | "source" => "registry.read",
			_ => return Ok(false),
		};
		if digest(&serde_json::to_value(&entry)?) != definition.digest
			|| !source
				.decide(&source.catalog_resource(&entry), action)
				.await?
		{
			return Ok(false);
		}
	}
	Ok(true)
}

#[cfg(test)]
mod tests;
