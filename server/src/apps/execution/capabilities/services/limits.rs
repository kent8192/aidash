//! Operator-lowered content budgets, bounded by the published wire contracts.

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

pub use crate::apps::execution::capabilities::serializers::limits::ContentLimits;

#[cfg(test)]
#[path = "../tests/services_limits_tests.rs"]
mod tests;
