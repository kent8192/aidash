//! Unit tests for services::limits.
use super::*;
#[rstest::fixture]
fn profile(#[default(1 << 30)] bytes: u64) -> crate::capabilities::Profile {
	crate::capabilities::Profile {
		working_bytes: bytes,
		..Default::default()
	}
}

#[rstest::rstest]
#[case(1 << 30, true)]
#[case((1 << 30) + 1, false)]
fn working_quota_cannot_exceed_runner_export_ceiling(
	#[case] bytes: u64,
	#[case] valid: bool,
	#[with(bytes)] profile: crate::capabilities::Profile,
) {
	assert_eq!(profile.working_bytes, bytes);
	assert_eq!(crate::capabilities::Runtime::new(profile).is_ok(), valid);
}

#[rstest::rstest]
fn defaults_and_lower_content_limits_preserve_wire_ceilings() {
	let limits = ContentLimits::default();
	assert!(limits.valid());
	let value = serde_json::to_value(&limits).unwrap();
	assert_eq!(
		value,
		serde_json::json!({"command_bytes":65536,"patch_bytes":262144,"search_matches":50,"search_bytes":32768,"search_seconds":5,"read_bytes":16384,"skill_files":64,"skill_bytes":256000,"reference_files":8,"reference_bytes":10485760,"reference_pages":200,"reference_text_bytes":65536,"share_files":64,"share_file_bytes":104857600,"share_bytes":268435456})
	);
	for key in value.as_object().unwrap().keys() {
		let mut invalid = value.clone();
		invalid[key] = serde_json::json!(0);
		assert!(
			!serde_json::from_value::<ContentLimits>(invalid)
				.unwrap()
				.valid(),
			"{key}"
		);
		let mut invalid = value.clone();
		invalid[key] = serde_json::json!(value[key].as_u64().unwrap() + 1);
		assert!(
			!serde_json::from_value::<ContentLimits>(invalid)
				.unwrap()
				.valid(),
			"{key}"
		);
	}
	let lower: ContentLimits = serde_json::from_value(
		serde_json::json!({"read_bytes":64,"reference_text_bytes":1024,"share_files":1}),
	)
	.unwrap();
	assert!(lower.valid());
}
