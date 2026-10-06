//! Reauthorize polling, retained state and each bounded recipient frame.
use super::{distribution, installations};
use crate::{Error, Result, ports::marketplace::EventScope};
use aidash_domain::{
	Event, marketplace::definitions::reference, marketplace::events::recipient_event,
	registry::Entry,
};
pub async fn visible(scope: &mut dyn EventScope, event: &Event, node: &str) -> Result<bool> {
	if event.kind == "marketplace.audit" {
		return Ok(false);
	}
	match scope.compatibility_ready().await {
		Ok(()) => {}
		Err(Error::Forbidden) => return Ok(false),
		Err(error) => return Err(error),
	}
	let result = async {
		if let Some(id) = event.data["installation"].as_str() {
			if event.data["tenant"].as_str() != Some(scope.tenant()) {
				return Err(Error::Forbidden);
			}
			installations::view(scope, id, event.data["revision"].as_i64(), node).await?;
		} else if let Some(key) = event.data["key"].as_str() {
			let version = distribution::load(scope, key, "marketplace.read").await?;
			distribution::readable(scope, &version, node).await?;
		} else {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	.await;
	match result {
		Ok(()) => Ok(true),
		Err(Error::Forbidden) => Ok(false),
		Err(error) => Err(error),
	}
}
pub async fn poll_events(
	scope: &mut dyn EventScope,
	events: Vec<Event>,
	node: &str,
) -> Result<Vec<Event>> {
	let mut output = vec![];
	for event in events {
		let allowed = if event.kind.starts_with("marketplace.") {
			visible(scope, &event, node).await?
		} else if let Some(workspace) = event.workspace_id {
			scope.workspace_allowed(workspace, "workspace.read").await?
				&& scope
					.workspace_allowed(workspace, "workspace.events")
					.await? && scope.workspace_event_visible(&event).await?
		} else {
			false
		};
		if allowed {
			output.push(recipient_event(event));
		}
	}
	Ok(output)
}
/// State already includes authorized workspace events; Marketplace events still
/// require current distribution authority and recipient projection.
pub async fn state_events(
	scope: &mut dyn EventScope,
	events: Vec<Event>,
	node: &str,
) -> Result<Vec<Event>> {
	let mut output = vec![];
	for event in events {
		if !event.kind.starts_with("marketplace.") || visible(scope, &event, node).await? {
			output.push(recipient_event(event));
		}
	}
	Ok(output)
}
async fn entries(
	scope: &mut dyn EventScope,
	input: Vec<Entry>,
	single: bool,
	gate_before_read: bool,
	_node: &str,
) -> Result<Vec<Entry>> {
	let mut output = vec![];
	for entry in input {
		let checked = async {
			if gate_before_read && entry.installation.is_some() {
				scope.compatibility_ready().await?;
			}
			let entry = scope.catalog(&reference(&entry), "registry.read").await?;
			installations::check_pinned(scope, &entry).await?;
			if !single && !installations::active(scope, &entry).await? {
				return Err(Error::Forbidden);
			}
			Ok(entry)
		}
		.await;
		match checked {
			Ok(entry) => output.push(entry),
			Err(Error::Forbidden) if !single => {}
			Err(error) => return Err(error),
		}
	}
	Ok(output)
}
pub async fn registry_entries(
	scope: &mut dyn EventScope,
	input: Vec<Entry>,
	single: bool,
	node: &str,
) -> Result<Vec<Entry>> {
	entries(scope, input, single, true, node).await
}
pub async fn state_registry(
	scope: &mut dyn EventScope,
	input: Vec<Entry>,
	node: &str,
) -> Result<Vec<Entry>> {
	entries(scope, input, false, false, node).await
}
/// No database lease survives the downstream SSE yield; the caller commits the
/// bounded queue insertion while authority is still held.
pub async fn frame(
	scope: &mut dyn EventScope,
	event: &Event,
	node: &str,
) -> Result<Option<String>> {
	if !visible(scope, event, node).await? {
		return Ok(None);
	}
	let payload = recipient_event(event.clone()).cloud_event().to_string();
	if payload.len() > 65_536 {
		return Err(Error::Forbidden);
	}
	Ok(Some(payload))
}
#[cfg(test)]
mod tests;
