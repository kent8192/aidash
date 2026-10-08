//! Current idle authority, bounded replay and per-frame authorization share policy rules.
use super::projection::EVENT_SCAN_LIMIT;
use crate::{
	Error, Result,
	authorization::Snapshot,
	ports::authorization::stream::{StreamAuthorityStore, StreamSession},
};
use aidash_domain::{
	Event,
	identity::authority::{DashboardStatusFailure, dashboard_status, enabled},
	policy::{Evaluation, Resource},
};
use serde_json::json;
use uuid::Uuid;
pub async fn authority(scope: &dyn StreamAuthorityStore, workspace: Option<Uuid>) -> Result<()> {
	let current = scope.current(workspace).await?.ok_or(Error::Unauthorized)?;
	if current.mapping_id.is_some() {
		if current.mapping_enabled != Some(true) {
			return Err(Error::Forbidden);
		}
		dashboard_status(
			current
				.issuer
				.map(|issuer| (issuer, current.last_valid_at, current.disabled_at)),
			scope.now(),
			scope.google_issuer(),
		)
		.map_err(|reason| match reason {
			DashboardStatusFailure::Forbidden => Error::Forbidden,
			DashboardStatusFailure::Unavailable => Error::IdentityStatusUnavailable,
		})?;
	}
	let snapshot = Snapshot {
		revision: current.revision,
		bundle: serde_json::from_value(current.document)?,
	};
	if !enabled(&snapshot.bundle, scope.subject()) {
		return Err(Error::Forbidden);
	}
	if let Some(id) = workspace {
		let owner = current.owner_subject.ok_or(Error::Forbidden)?;
		for action in ["workspace.read", "workspace.events"] {
			let input = Evaluation {
				subject: scope.subject().into(),
				action: action.into(),
				resource: Resource {
					tenant: scope.tenant().into(),
					kind: "workspace".into(),
					id: id.to_string(),
					attributes: json!({"owner":owner,"workspace_id":id}),
				},
				environment: json!({"node_id":scope.node_id(),"transport":"api"}),
			};
			if !snapshot.bundle.evaluate(&input).allowed {
				return Err(Error::Forbidden);
			}
		}
	}
	Ok(())
}
pub async fn stream_page<F>(
	scope: &mut dyn StreamSession,
	after: i64,
	workspace: Option<Uuid>,
	on_query: F,
) -> Result<(Vec<Event>, i64, bool)>
where
	F: FnOnce(),
{
	let visible = scope.event_workspaces(workspace).await?;
	if workspace.is_some() && visible.is_empty() {
		return Err(Error::Forbidden);
	}
	on_query();
	let batch = scope.stream_rows(after, workspace, &visible).await?;
	let more = batch.len() == 100;
	let scanned = batch.last().map_or(after, |event| event.sequence);
	let mut events = vec![];
	for event in batch {
		if scope.event_visible(&event).await? {
			events.push(event);
		}
	}
	Ok((events, scanned, more))
}
pub async fn read_events(
	scope: &mut dyn StreamSession,
	after: i64,
	workspace: Option<Uuid>,
	limit: i64,
) -> Result<(Vec<Event>, i64)> {
	let visible = if let Some(id) = workspace {
		scope.require_workspace(id, "workspace.read").await?;
		scope.require_workspace(id, "workspace.events").await?;
		vec![id]
	} else {
		let mut visible = vec![];
		for id in scope.visible("workspace.read").await? {
			if scope.allowed(id, "workspace.events").await? {
				visible.push(id);
			}
		}
		visible
	};
	let limit = limit.clamp(1, 1000) as usize;
	let mut cursor = after.max(0);
	let mut result = vec![];
	let mut scanned = 0;
	while scanned < EVENT_SCAN_LIMIT {
		let page_size = (EVENT_SCAN_LIMIT - scanned).min(500);
		let batch = scope
			.read_rows(cursor, workspace, &visible, page_size)
			.await?;
		let exhausted = batch.len() < page_size;
		scanned += batch.len();
		for event in batch {
			cursor = event.sequence;
			if scope.event_visible(&event).await? {
				result.push(event);
			}
			if result.len() == limit {
				return Ok((result, cursor));
			}
		}
		if exhausted {
			break;
		}
	}
	Ok((result, cursor))
}

pub async fn can_emit(scope: &mut dyn StreamSession, event: &Event) -> Result<bool> {
	if event.workspace_id.is_none()
		&& (event.kind.starts_with("provider_credential.")
			|| event.kind.starts_with("provider_credential_binding."))
	{
		return scope.event_visible(event).await;
	}
	let Some(id) = event.workspace_id else {
		return Ok(false);
	};
	Ok(!scope.event_workspaces(Some(id)).await?.is_empty() && scope.event_visible(event).await?)
}
#[cfg(test)]
mod tests;
