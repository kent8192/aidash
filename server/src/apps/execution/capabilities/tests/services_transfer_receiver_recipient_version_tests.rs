//! Unit tests for services::transfer::receiver.
use super::{MAX_RECIPIENT_VERSIONS_PER_AREA, cap_recipient_versions};

#[test]
fn recipient_versions_are_capped_and_report_truncation() {
	for (count, expected_truncated) in [(0, false), (50, false), (51, true)] {
		let mut versions = (0..count).map(|i| format!("0.0.{i}")).collect::<Vec<_>>();
		assert_eq!(cap_recipient_versions(&mut versions), expected_truncated);
		assert_eq!(versions.len(), count.min(MAX_RECIPIENT_VERSIONS_PER_AREA));
	}
}
