use super::*;
use crate::capabilities::operations::FileScope;
use rstest::rstest;

fn file(id: u128, path: &str, size: u64, media_type: &str) -> MountedFile {
	MountedFile {
		file_id: Uuid::from_u128(id),
		path: path.into(),
		digest: "a".repeat(64),
		size,
		media_type: media_type.into(),
		scope: FileScope::Working,
		provenance: serde_json::json!({"kind":"patch"}),
	}
}

fn holder(message: u128) -> Holder {
	Holder::Message {
		message_id: Uuid::from_u128(message),
	}
}

fn hold(holder: Holder, files: &[MountedFile], revision: i64) -> RetentionHold {
	RetentionHold {
		tenant: "acme".into(),
		area_id: Uuid::from_u128(1),
		revision,
		holder,
		files: files.iter().map(RetainedFile::from).collect(),
	}
}

#[rstest]
#[case("text/html", "page.txt", true)]
#[case("text/html; charset=utf-8", "page.txt", true)]
#[case("application/octet-stream", "index.html", true)]
#[case("text/plain", "index.HTM", true)]
#[case("text/markdown", "notes.md", false)]
fn html_is_decided_by_media_type_or_suffix(
	#[case] media_type: &str,
	#[case] path: &str,
	#[case] expected: bool,
) {
	assert_eq!(is_html(media_type, path), expected);
}

#[test]
fn a_plain_version_retains_only_itself() {
	let page = file(2, "notes.txt", 10, "text/plain");
	let manifest = vec![page.clone(), file(3, "other.txt", 20, "text/plain")];
	let retained = retained_files(&page, &manifest);
	assert_eq!(retained, vec![RetainedFile::from(&page)]);
}

#[test]
fn an_html_version_retains_its_whole_revision() {
	let page = file(2, "index.html", 10, "text/html");
	let sibling = file(3, "assets/app.js", 20, "text/javascript");
	let reference = file(4, "cited.txt", 30, "text/plain");
	let manifest = vec![page.clone(), sibling.clone(), reference.clone()];
	let retained = retained_files(&page, &manifest);
	assert_eq!(
		retained,
		vec![
			RetainedFile::from(&page),
			RetainedFile::from(&sibling),
			RetainedFile::from(&reference),
		]
	);
	assert!(
		retained_files(&page, &manifest)
			.iter()
			.any(|f| f.file_id == sibling.file_id)
	);
}

#[test]
fn a_hold_covers_the_versions_it_names() {
	let held = hold(holder(9), &[file(2, "a.txt", 1, "text/plain")], 4);
	assert!(held.covers(Uuid::from_u128(2)));
	assert!(!held.covers(Uuid::from_u128(3)));
}

#[test]
fn retained_bytes_count_only_versions_that_left_the_manifest_once() {
	let superseded = file(2, "old.txt", 40, "text/plain");
	let current = vec![file(3, "old.txt", 41, "text/plain")];
	let first = hold(holder(9), std::slice::from_ref(&superseded), 3);
	let second = hold(holder(10), std::slice::from_ref(&superseded), 3);
	let holds = vec![first, second];
	assert_eq!(
		retained_bytes(&holds, &current, Utc::now()),
		40,
		"a version held twice is one object"
	);
}

#[test]
fn a_version_still_in_the_manifest_is_not_retained_bytes() {
	let held = file(2, "live.txt", 40, "text/plain");
	let current = vec![held.clone()];
	let holds = vec![hold(holder(9), std::slice::from_ref(&held), 1)];
	assert_eq!(retained_bytes(&holds, &current, Utc::now()), 0);
}

#[test]
fn an_expired_federation_hold_stops_keeping_bytes() {
	let superseded = file(2, "old.txt", 40, "text/plain");
	let current = vec![file(3, "new.txt", 1, "text/plain")];
	let expired = Holder::Federation {
		peer: "peer.example".into(),
		reference_id: "ref".into(),
		expires_at: Utc::now() - chrono::Duration::seconds(1),
	};
	let holds = vec![hold(expired, std::slice::from_ref(&superseded), 3)];
	assert_eq!(retained_bytes(&holds, &current, Utc::now()), 0);
}

#[rstest]
#[case(100, 0, 100, true)]
#[case(101, 0, 100, false)]
#[case(101, 1, 100, true)]
#[case(0, 0, 0, true)]
fn retention_discounts_reclaimable_bytes(
	#[case] used: u64,
	#[case] reclaimable: u64,
	#[case] limit: u64,
	#[case] expected: bool,
) {
	assert_eq!(permits_retention(used, reclaimable, limit), expected);
}

#[rstest]
#[case(80, 20, 100, true)]
#[case(81, 20, 100, false)]
fn a_working_area_counts_retained_bytes(
	#[case] manifest: u64,
	#[case] retained: u64,
	#[case] limit: u64,
	#[case] expected: bool,
) {
	assert_eq!(area_permits_retention(manifest, retained, limit), expected);
}

#[test]
fn an_expired_holder_is_the_only_holder_that_expires() {
	let now = Utc::now();
	assert!(!holder(9).expired(now));
	assert!(
		!Holder::LivePreview {
			live_preview_id: Uuid::from_u128(2)
		}
		.expired(now)
	);
}

#[test]
fn the_same_holder_ignores_a_recomputed_federation_expiry() {
	let message = holder(9);
	assert!(message.same(&holder(9)));
	assert!(!message.same(&holder(10)));
	let first = Holder::Federation {
		peer: "peer".into(),
		reference_id: "ref".into(),
		expires_at: Utc::now(),
	};
	let second = Holder::Federation {
		peer: "peer".into(),
		reference_id: "ref".into(),
		expires_at: Utc::now() + chrono::Duration::hours(1),
	};
	assert!(first.same(&second));
	assert_eq!(first.kind(), "federation");
	assert_eq!(first.peer(), Some("peer"));
}
