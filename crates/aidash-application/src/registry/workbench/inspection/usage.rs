//! Usage bounds count authorized Runs, including hidden-page scanning on one audit lease.
use crate::{
	Error, Result,
	ports::registry::workbench::inspection::{InspectionRepository, InspectionScope},
};
use aidash_domain::{
	RunMetadata,
	identity::Principal,
	registry::{
		EntityRef,
		workbench::inspection::{Inspection, TestEvidence, WorkspaceUse},
	},
};
use chrono::Utc;
use std::collections::BTreeMap;
use uuid::Uuid;
async fn run_visible(scope: &mut dyn InspectionScope, run: &RunMetadata) -> Result<bool> {
	if scope.principal() == Principal::Operator {
		return Ok(true);
	}
	let result = async {
		let workspace = scope.workspace(run.workspace_id).await?;
		if !scope.decide(&workspace, "workspace.read").await? {
			return Ok(false);
		}
		scope.run_visible(run).await
	}
	.await;
	match result {
		Err(Error::Forbidden) => Ok(false),
		other => other,
	}
}
async fn visible_runs(
	scope: &mut dyn InspectionScope,
	reference: &EntityRef,
) -> Result<Vec<RunMetadata>> {
	let mut visible = Vec::new();
	let mut cursor = None;
	loop {
		let rows = scope.agent_runs(reference, cursor).await?;
		let more = rows.len() == 501;
		cursor = rows.last().map(|run| (run.updated_at, run.id));
		for run in rows {
			if run_visible(scope, &run).await? {
				visible.push(run);
			}
			if visible.len() == 501 {
				break;
			}
		}
		if visible.len() == 501 || !more {
			break;
		}
	}
	Ok(visible)
}
async fn add_use(
	scope: &mut dyn InspectionScope,
	uses: &mut BTreeMap<Uuid, WorkspaceUse>,
	run: &RunMetadata,
) -> Result<()> {
	let active = !run.phase().is_terminal();
	if let Some(existing) = uses.get_mut(&run.workspace_id) {
		existing.current |= active;
		if run.updated_at > existing.latest_run_at {
			existing.latest_run_at = run.updated_at;
		}
		return Ok(());
	}
	if let Some(title) = scope.workspace_title(run.workspace_id).await? {
		uses.insert(
			run.workspace_id,
			WorkspaceUse {
				workspace_id: run.workspace_id,
				title,
				current: active,
				latest_run_at: run.updated_at,
			},
		);
	}
	Ok(())
}
pub async fn inspect(
	repository: &dyn InspectionRepository,
	reference: EntityRef,
) -> Result<Inspection> {
	let mut scope = repository.begin().await?;
	scope.require_inspection(&reference).await?;
	let entry = scope.effective(&reference).await?;
	if entry.kind != "agent" {
		return Err(Error::NotFound("agent version".into()));
	}
	let runs = visible_runs(scope.as_mut(), &reference).await?;
	let truncated = runs.len() > 500;
	let mut uses = BTreeMap::new();
	for run in runs.into_iter().take(500) {
		add_use(scope.as_mut(), &mut uses, &run).await?;
	}
	let mut test_evidence = Vec::new();
	let mut test_evidence_truncated = false;
	for registration in scope.registrations(&reference).await? {
		let draft = scope.draft(registration.draft_id).await?;
		match scope.authorize_draft(&draft).await {
			Ok(()) => {}
			Err(Error::Forbidden) => continue,
			Err(error) => return Err(error),
		}
		let sessions = scope
			.test_observations(registration.draft_id, registration.revision)
			.await?;
		test_evidence_truncated = sessions.len() > 100;
		for session in sessions.into_iter().take(100) {
			test_evidence.push(TestEvidence {
				session_id: session.id,
				draft_revision: registration.revision,
				mode: session.scenario["mode"]
					.as_str()
					.unwrap_or("unknown")
					.into(),
				profile_id: session.scenario["profile_id"].as_str().map(str::to_owned),
				profile_revision: session.scenario["profile_revision"].as_i64(),
				status: session.status,
				usage: session.usage,
				created_at: session.created_at,
				expires_at: session.expires_at,
				expired_at: session.expired_at,
			});
		}
	}
	scope
		.finish(Inspection {
			entry,
			source_node: repository.node_id().into(),
			observed_at: Utc::now(),
			workspaces: uses.into_values().collect(),
			usage_truncated: truncated,
			test_evidence,
			test_evidence_truncated,
			external_assessment_available: false,
		})
		.await
}
#[cfg(test)]
mod tests;
