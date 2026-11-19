//! Unit tests for services::transfer::receiver.
use super::*;

#[test]
fn recipient_versions_are_capped_and_report_truncation() {
	for (count, expected_truncated) in [(0, false), (50, false), (51, true)] {
		let mut versions = (0..count).map(|i| format!("0.0.{i}")).collect::<Vec<_>>();
		assert_eq!(cap_recipient_versions(&mut versions), expected_truncated);
		assert_eq!(versions.len(), count.min(MAX_RECIPIENT_VERSIONS_PER_AREA));
	}
}

use crate::capabilities::{
	operations::{FileScope, MountedFile},
	sharing::Recipient,
};
use crate::registry::EntityRef;
use uuid::Uuid;
fn manifest(now: DateTime<Utc>) -> Description {
	let files = vec![MountedFile {
		file_id: Uuid::new_v4(),
		path: "report.txt".into(),
		digest: "a".repeat(64),
		size: 5,
		media_type: "text/plain".into(),
		scope: FileScope::Working,
		provenance: Value::Null,
	}];
	let digest = crate::registry::rules::digest(&json!(files));
	Description {
		protocol: "file-transfer/1".into(),
		transfer_id: Uuid::new_v4(),
		source_node: "source".into(),
		target: Recipient {
			node_id: "target".into(),
			agent_id: "recipient".into(),
			agent_version: "v1".into(),
			thread_id: Uuid::new_v4(),
		},
		source_tenant: "tenant".into(),
		source_subject: "owner".into(),
		source_agent: EntityRef {
			id: "sender".into(),
			version: "v1".into(),
		},
		input_digest: "input".into(),
		manifest_digest: digest,
		files,
		expires_at: now + Duration::hours(1),
	}
}
fn limits() -> ManifestLimits {
	ManifestLimits {
		files: 4,
		file_bytes: 8,
		bytes: 12,
	}
}
#[rstest::rstest]
fn a_complete_bounded_manifest_reserves_its_exact_bytes() {
	let now = Utc::now();
	assert_eq!(validate(&manifest(now), &limits(), now).unwrap(), 5);
}
#[rstest::rstest]
#[case(0)]
#[case(-1)]
#[case(25)]
fn unsupported_expiry_is_rejected(#[case] hours: i64) {
	let now = Utc::now();
	let mut d = manifest(now);
	d.expires_at = now + Duration::hours(hours);
	assert!(
		matches!(validate(&d,&limits(),now),Err(Error::Invalid(code)) if code=="TRANSFER_MANIFEST_LIMIT")
	);
}
#[rstest::rstest]
fn duplicate_paths_cannot_reserve_a_second_snapshot() {
	let now = Utc::now();
	let mut d = manifest(now);
	d.files.push(d.files[0].clone());
	assert!(
		matches!(validate(&d,&limits(),now),Err(Error::Invalid(code)) if code=="TRANSFER_MANIFEST_LIMIT")
	);
}
#[rstest::rstest]
fn altered_metadata_cannot_keep_a_stale_manifest_digest() {
	let now = Utc::now();
	let mut d = manifest(now);
	d.files[0].media_type = "image/png".into();
	assert!(
		matches!(validate(&d,&limits(),now),Err(Error::Invalid(code)) if code=="TRANSFER_MANIFEST_INTEGRITY")
	);
}
#[rstest::rstest]
fn chunk_digest_is_the_exact_sha256_of_received_bytes() {
	assert_eq!(
		chunk_digest(b"abc"),
		"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
	);
}
