//! Marketplace event payloads have no manifests. Their referenced resource is
//! still reauthorized at polling, replay and each bounded frame handoff.
use super::{distribution, installations, storage};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	domain::Event,
	store::Store,
};
use axum::response::Response;

pub(super) fn recipient_event(mut event: Event) -> Event {
	// Older durable events may still contain the publisher's private subject.
	if matches!(
		event.kind.as_str(),
		"marketplace.published" | "marketplace.distribution_changed"
	) && let Some(data) = event.data.as_object_mut()
	{
		data.remove("actor");
	}
	event
}

pub(crate) async fn visible(access: &mut Access, event: &Event, node: &str) -> Result<bool> {
	if event.kind == "marketplace.audit" {
		return Ok(false);
	}
	match storage::gate(&mut access.tx).await {
		Ok(()) => {}
		Err(Error::Forbidden) => return Ok(false),
		Err(error) => return Err(error),
	}
	let result = async {
		if let Some(id) = event.data["installation"].as_str() {
			if event.data["tenant"].as_str() != Some(&access.identity.tenant) {
				return Err(Error::Forbidden);
			}
			installations::view(access, id, event.data["revision"].as_i64(), node).await?;
		} else if let Some(key) = event.data["key"].as_str() {
			let version = distribution::load(access, key, "marketplace.read").await?;
			distribution::readable(access, &version, node).await?;
		} else {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	.await;
	match result {
		Ok(()) => Ok(true),
		Err(Error::Forbidden) => Ok(false),
		Err(e) => Err(e),
	}
}
pub(crate) async fn response(
	store: &Store,
	identity: &SubjectIdentity,
	events: Vec<Event>,
) -> Result<Response> {
	let mut access = Access::begin(store, identity).await?;
	// Acquire distribution before any catalog reader, including preceding
	// workspace events in the same response. Initial candidate filtering is
	// repeated here under this lease immediately before disclosure.
	storage::lock(&mut access.tx, false).await?;
	let result = async {
		let mut output = vec![];
		for event in events {
			let allowed = if event.kind.starts_with("marketplace.") {
				visible(&mut access, &event, &store.node_id).await?
			} else if let Some(workspace) = event.workspace_id {
				access.allowed(workspace, "workspace.read").await?
					&& access.allowed(workspace, "workspace.events").await?
					&& access.event_visible(&event).await?
			} else {
				false
			};
			if allowed {
				output.push(recipient_event(event));
			}
		}
		Ok(output)
	}
	.await;
	super::api::handoff(store, access, result).await
}
pub(crate) async fn frame(
	store: &Store,
	identity: &SubjectIdentity,
	event: &Event,
) -> Result<Option<String>> {
	let mut access = Access::begin(store, identity).await?;
	// Acquire distribution before any catalog reader, including preceding
	// workspace events in the same response. Initial candidate filtering is
	// repeated here under this lease immediately before disclosure.
	storage::lock(&mut access.tx, false).await?;
	let result = async {
		if !visible(&mut access, event, &store.node_id).await? {
			return Ok(None);
		}
		let payload = recipient_event(event.clone()).cloud_event().to_string();
		if payload.len() > 65_536 {
			return Err(Error::Forbidden);
		}
		storage::credential_current(&mut access).await?;
		let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
		sender.try_send(payload).map_err(|_| Error::Forbidden)?;
		// The immutable frame has entered the bounded disclosure queue while
		// revocation is still blocked. No lock survives a downstream SSE yield.
		Ok(receiver.try_recv().ok())
	}
	.await;
	access.finish(result).await
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	use uuid::Uuid;

	#[test]
	fn recipient_replay_removes_legacy_publisher_subject() {
		let event = Event {
			sequence: 1,
			id: Uuid::nil(),
			node_id: "node".into(),
			workspace_id: None,
			kind: "marketplace.published".into(),
			data: json!({"key":"package","tenant":"owner","actor":"private-subject"}),
			created_at: chrono::Utc::now(),
		};
		let received = recipient_event(event);
		assert_eq!(received.data, json!({"key":"package","tenant":"owner"}));
	}
}
