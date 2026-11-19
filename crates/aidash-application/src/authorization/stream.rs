//! Current idle authority, bounded replay and per-frame authorization share policy rules.
use super::projection::EVENT_SCAN_LIMIT;
use crate::{
	Error, Result,
	authorization::Snapshot,
	ports::authorization::{
		stream::{StreamAuthorityStore, StreamSession},
		visibility::RunVerdicts,
	},
};
use aidash_domain::{
	Event,
	identity::authority::{DashboardStatusFailure, dashboard_status, enabled},
	policy::{Evaluation, Resource},
	unconfirmed::{Kind, Tally},
};
use serde_json::json;
use uuid::Uuid;
pub async fn authority(scope: &dyn StreamAuthorityStore, thread: Option<Uuid>) -> Result<()> {
	let current = scope.current(thread).await?.ok_or(Error::Unauthorized)?;
	if current.mapping_id.is_some() {
		if current.mapping_enabled != Some(true) {
			return Err(Error::Forbidden);
		}
		dashboard_status(
			current
				.issuer
				.map(|issuer| (issuer, current.last_valid_at, current.disabled_at)),
			scope.now(),
			|issuer| scope.reports_status(issuer),
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
	if let Some(id) = thread {
		let owner = crate::ports::authorization::threads::ThreadOwner {
			owner: current.owner_subject.ok_or(Error::Forbidden)?,
			duty_id: current.duty_id,
		};
		for action in ["thread.read", "thread.events"] {
			let input = Evaluation {
				subject: scope.subject().into(),
				action: action.into(),
				resource: Resource {
					tenant: scope.tenant().into(),
					kind: "thread".into(),
					id: id.to_string(),
					attributes: super::threads::attributes(id, &owner),
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
	thread: Option<Uuid>,
	on_query: F,
) -> Result<(Vec<Event>, i64, bool)>
where
	F: FnOnce(),
{
	let visible = scope.event_threads(thread).await?;
	if thread.is_some() && visible.is_empty() {
		return Err(Error::Forbidden);
	}
	on_query();
	let batch = scope.stream_rows(after, thread, &visible).await?;
	let more = batch.len() == 100;
	let scanned = batch.last().map_or(after, |event| event.sequence);
	let mut events = vec![];
	let mut runs = RunVerdicts::default();
	for event in batch {
		if scope.event_visible(&event, &mut runs).await? {
			events.push(event);
		}
	}
	Ok((events, scanned, more))
}
pub async fn read_events(
	scope: &mut dyn StreamSession,
	after: i64,
	thread: Option<Uuid>,
	limit: i64,
) -> Result<(Vec<Event>, i64, Tally)> {
	let visible = if let Some(id) = thread {
		scope.require_thread(id, "thread.read").await?;
		scope.require_thread(id, "thread.events").await?;
		vec![id]
	} else {
		let mut visible = vec![];
		for id in scope.visible("thread.read").await? {
			if scope.allowed(id, "thread.events").await? {
				visible.push(id);
			}
		}
		visible
	};
	let limit = limit.clamp(1, 1000) as usize;
	let mut cursor = after.max(0);
	let mut result = vec![];
	let mut unconfirmed = Tally::new();
	let mut scanned = 0;
	let mut runs = RunVerdicts::default();
	while scanned < EVENT_SCAN_LIMIT {
		let page_size = (EVENT_SCAN_LIMIT - scanned).min(500);
		let batch = scope.read_rows(cursor, thread, &visible, page_size).await?;
		let exhausted = batch.len() < page_size;
		scanned += batch.len();
		for event in batch {
			cursor = event.sequence;
			scope.begin_row();
			let visible = scope.event_visible(&event, &mut runs).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle_in(silence, Kind::Event, event.thread_id, visible) {
				result.push(event);
			}
			if result.len() == limit {
				return Ok((result, cursor, unconfirmed));
			}
		}
		if exhausted {
			break;
		}
	}
	Ok((result, cursor, unconfirmed))
}

pub async fn can_emit(scope: &mut dyn StreamSession, event: &Event) -> Result<bool> {
	if event.thread_id.is_none()
		&& (event.kind.starts_with("provider_credential.")
			|| event.kind.starts_with("provider_credential_binding."))
	{
		return scope
			.event_visible(event, &mut RunVerdicts::default())
			.await;
	}
	let Some(id) = event.thread_id else {
		return Ok(false);
	};
	Ok(!scope.event_threads(Some(id)).await?.is_empty()
		&& scope
			.event_visible(event, &mut RunVerdicts::default())
			.await?)
}
#[cfg(test)]
mod tests;
