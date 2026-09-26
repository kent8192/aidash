//! Operator-lowered content budgets, bounded by the published wire contracts.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContentLimits {
	pub command_bytes: usize,
	pub patch_bytes: usize,
	pub search_matches: usize,
	pub search_bytes: usize,
	pub search_seconds: u64,
	pub read_bytes: usize,
	pub skill_files: usize,
	pub skill_bytes: usize,
	pub reference_files: usize,
	pub reference_bytes: u64,
	pub reference_pages: usize,
	pub reference_text_bytes: usize,
	pub share_files: usize,
	pub share_file_bytes: u64,
	pub share_bytes: u64,
}
impl Default for ContentLimits {
	fn default() -> Self {
		Self {
			command_bytes: 65536,
			patch_bytes: 256 << 10,
			search_matches: 50,
			search_bytes: 32 << 10,
			search_seconds: 5,
			read_bytes: 16 << 10,
			skill_files: 64,
			skill_bytes: 256_000,
			reference_files: 8,
			reference_bytes: 10 << 20,
			reference_pages: 200,
			reference_text_bytes: 64 << 10,
			share_files: 64,
			share_file_bytes: 100 << 20,
			share_bytes: 256 << 20,
		}
	}
}
impl ContentLimits {
	pub(crate) fn valid(&self) -> bool {
		let current = serde_json::to_value(self).expect("numeric limits serialize");
		let maximum = serde_json::to_value(Self::default()).expect("numeric limits serialize");
		current
			.as_object()
			.expect("limits are an object")
			.iter()
			.all(|(key, value)| {
				let number = value.as_u64().expect("nonnegative limit");
				number > 0 && number <= maximum[key].as_u64().expect("same numeric field")
			}) && self.search_bytes >= 4096
			&& self.read_bytes >= 4
			&& self.reference_text_bytes >= 4
	}
}

#[cfg(test)]
mod tests {
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

	#[test]
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
}
