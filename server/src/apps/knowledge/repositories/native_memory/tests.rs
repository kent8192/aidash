use super::canonical;
use aidash_domain::memory::{Content, Kind, Learning, TimeRange, Verification};
use aidash_domain::registry::{EntityRef, bindings::memory_defaults};
use chrono::{TimeZone, Utc};
use rstest::rstest;

fn content(verification: Verification) -> Content {
	Content {
		mental_model: None,
		text: "Tag releases from main.".into(),
		kind: Kind::World,
		learning: Learning::Fact,
		verification,
		occurred: Some(TimeRange {
			start: Utc.timestamp_opt(1_700_000_000, 123_456_789).unwrap(),
			end: Utc.timestamp_opt(1_700_000_001, 987_654_321).unwrap(),
		}),
		entities: vec![],
		evidence: vec![],
		links: vec![],
	}
}

fn provider(id: &str, version: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: version.into(),
	}
}

#[rstest]
#[case::built_in_records_unverified(
	provider(memory_defaults::MEMORY_ID, memory_defaults::VERSION),
	Verification::Unverified,
	Verification::Recorded
)]
#[case::built_in_keeps_contradicted(
	provider(memory_defaults::MEMORY_ID, memory_defaults::VERSION),
	Verification::Contradicted,
	Verification::Contradicted
)]
#[case::explicit_provider_keeps_unverified(
	provider("p", "1.0.0"),
	Verification::Unverified,
	Verification::Unverified
)]
fn canonical_content_is_what_a_changed_write_stores(
	#[case] provider: EntityRef,
	#[case] incoming: Verification,
	#[case] stored: Verification,
) {
	// Arrange
	let incoming = content(incoming);

	// Act
	let canonical = canonical(&incoming, &provider);

	// Assert: an identical write compares against this, so a repeat that omits
	// the built-in provider's `recorded` still matches the stored Unit (#127).
	assert_eq!(canonical.verification, stored);
	let range = canonical.occurred.unwrap();
	assert_eq!(range.start.timestamp_subsec_nanos(), 123_456_000);
	assert_eq!(range.end.timestamp_subsec_nanos(), 987_654_000);
}
