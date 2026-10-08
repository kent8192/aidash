//! Unit tests for services::broker.
use super::*;
#[rstest::rstest]
fn only_valid_node_metadata_can_wake_observers(service: Service) {
	let mut observer = service.register(None);
	for bytes in [b"not json".as_slice(), br#"{"specversion":"1.0","source":"aidash://other","id":"00000000-0000-0000-0000-000000000001"}"#] {
		service.hint(bytes, "aidash://local");
	}
	assert_eq!(observer.take_changes().0, 0);
	service.hint(br#"{"specversion":"1.0","source":"aidash://local","id":"00000000-0000-0000-0000-000000000001","subject":null,"sequence":9223372036854775807,"dataref":"http://untrusted.invalid"}"#, "aidash://local");
	assert_eq!(observer.take_changes().0, Reasons::NOTIFICATION.0);
	assert_eq!(service.snapshot().rejected_notifications, 2);
}

#[rstest::fixture]
fn service() -> Service {
	Service::new(Settings::default())
}
