//! Stable recipient event projection for durable replay.
use crate::Event;
pub fn recipient_event(mut event: Event) -> Event {
	// Older durable events can contain the publisher's private subject.
	if matches!(
		event.kind.as_str(),
		"marketplace.published" | "marketplace.distribution_changed"
	) && let Some(data) = event.data.as_object_mut()
	{
		data.remove("actor");
	}
	event
}
