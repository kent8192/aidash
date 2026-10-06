//! Marketplace event payloads have no manifests. Their referenced resource is
//! still reauthorized at polling, replay and each bounded frame handoff.
use super::storage;
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	domain::Event,
	store::Store,
};
use reinhardt::Response;

#[cfg(test)]
pub(crate) fn recipient_event(event: Event) -> Event {
	aidash_domain::marketplace::events::recipient_event(event)
}

pub(crate) async fn visible(access: &mut Access, event: &Event, node: &str) -> Result<bool> {
	aidash_application::marketplace::events::visible(
		&mut crate::bootstrap::marketplace_distribution_scope(access),
		event,
		node,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn response(
	store: &Store,
	identity: &SubjectIdentity,
	events: Vec<Event>,
) -> Result<Response> {
	let mut access = Access::begin(store, identity).await?;
	// Distribution precedes catalog reads; repeat candidate filtering at handoff.
	storage::lock(&mut access.tx, false).await?;
	let result = aidash_application::marketplace::events::poll_events(
		&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
		events,
		&store.node_id,
	)
	.await
	.map_err(Into::into);
	super::management::handoff(store, access, result).await
}
pub(crate) async fn frame(
	store: &Store,
	identity: &SubjectIdentity,
	event: &Event,
) -> Result<Option<String>> {
	let mut access = Access::begin(store, identity).await?;
	storage::lock(&mut access.tx, false).await?;
	let result = async {
		let Some(payload) = aidash_application::marketplace::events::frame(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			event,
			&store.node_id,
		)
		.await?
		else {
			return Ok(None);
		};
		storage::credential_current(&mut access).await?;
		let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
		sender.try_send(payload).map_err(|_| Error::Forbidden)?;
		// Commit the bounded insertion while revocation remains blocked.
		Ok(receiver.try_recv().ok())
	}
	.await;
	access.finish(result).await
}

#[cfg(test)]
#[path = "../tests/events.rs"]
mod tests;
