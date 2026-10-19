//! Marketplace events: their payloads, the recipients an audience change
//! addresses, and the projection a recipient reads.
use crate::{Event, marketplace::Audience};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const PUBLISHED: &str = "marketplace.published";
pub const DISTRIBUTION_CHANGED: &str = "marketplace.distribution_changed";

/// A package became available to the Tenant that reads this event.
pub fn published_payload(key: &str, owner: &str) -> Value {
	json!({"key":key,"tenant":owner})
}

/// The audience of a package changed to `revision`.
pub fn distribution_changed_payload(key: &str, owner: &str, revision: i64) -> Value {
	json!({"key":key,"tenant":owner,"revision":revision})
}

/// One event for the Tenants of an audience. An event belongs to one Tenant, so
/// each recipient is written its own copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Announcement {
	pub kind: &'static str,
	pub payload: Value,
	pub tenants: BTreeSet<String>,
}

/// What a change of `key`'s audience tells the Tenants other than its owner,
/// whose own events the owner's transaction writes. A Tenant that joins learns
/// that the package is published to it, and every member learns that the
/// audience changed. The payloads hold what a member reads in the package's
/// summary and detail: the key, the owner and the audience revision.
pub fn audience_announcements(
	key: &str,
	owner: &str,
	previous: &Audience,
	next: &Audience,
) -> Vec<Announcement> {
	let members = || {
		next.tenants
			.iter()
			.filter(|tenant| tenant.as_str() != owner)
	};
	let joined = members()
		.filter(|tenant| !previous.tenants.contains(*tenant))
		.cloned()
		.collect::<BTreeSet<_>>();
	let mut announcements = vec![];
	if !joined.is_empty() {
		announcements.push(Announcement {
			kind: PUBLISHED,
			payload: published_payload(key, owner),
			tenants: joined,
		});
	}
	let members = members().cloned().collect::<BTreeSet<_>>();
	if !members.is_empty() {
		announcements.push(Announcement {
			kind: DISTRIBUTION_CHANGED,
			payload: distribution_changed_payload(key, owner, next.revision),
			tenants: members,
		});
	}
	announcements
}

/// Stable recipient event projection for durable replay.
pub fn recipient_event(mut event: Event) -> Event {
	// Older durable events can contain the publisher's private subject.
	if matches!(event.kind.as_str(), PUBLISHED | DISTRIBUTION_CHANGED)
		&& let Some(data) = event.data.as_object_mut()
	{
		data.remove("actor");
	}
	event
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	fn audience(revision: i64, tenants: &[&str]) -> Audience {
		Audience {
			revision,
			tenants: tenants.iter().map(|tenant| (*tenant).to_owned()).collect(),
		}
	}

	fn recipients(announcement: &Announcement) -> Vec<&str> {
		announcement.tenants.iter().map(String::as_str).collect()
	}

	#[rstest]
	fn a_joining_tenant_learns_of_the_package_and_every_member_of_the_change() {
		// Arrange
		let previous = audience(2, &["owner", "kept"]);
		let next = audience(3, &["owner", "kept", "joined"]);
		// Act
		let announcements = audience_announcements("package", "owner", &previous, &next);
		// Assert
		assert_eq!(announcements.len(), 2);
		assert_eq!(announcements[0].kind, PUBLISHED);
		assert_eq!(recipients(&announcements[0]), ["joined"]);
		assert_eq!(
			announcements[0].payload,
			json!({"key":"package","tenant":"owner"})
		);
		assert_eq!(announcements[1].kind, DISTRIBUTION_CHANGED);
		assert_eq!(recipients(&announcements[1]), ["joined", "kept"]);
		assert_eq!(
			announcements[1].payload,
			json!({"key":"package","tenant":"owner","revision":3})
		);
	}

	#[rstest]
	#[case::only_the_owner_remains(audience(1, &["owner"]), audience(2, &["owner"]))]
	#[case::the_owner_is_the_audience(audience(1, &[]), audience(2, &["owner"]))]
	fn the_owner_is_never_addressed(#[case] previous: Audience, #[case] next: Audience) {
		assert!(audience_announcements("package", "owner", &previous, &next).is_empty());
	}

	#[rstest]
	fn a_withdrawn_tenant_is_not_addressed() {
		// Arrange
		let previous = audience(2, &["owner", "withdrawn", "kept"]);
		let next = audience(3, &["owner", "kept"]);
		// Act
		let announcements = audience_announcements("package", "owner", &previous, &next);
		// Assert
		assert_eq!(announcements.len(), 1);
		assert_eq!(announcements[0].kind, DISTRIBUTION_CHANGED);
		assert_eq!(recipients(&announcements[0]), ["kept"]);
	}
}
