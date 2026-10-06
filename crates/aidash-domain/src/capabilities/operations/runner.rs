//! Deployment probes must match the operator's immutable execution ceilings.
use serde_json::Value;
pub struct HealthProfile {
	pub image: String,
	pub runtime_class: String,
	pub cpu: u32,
	pub memory_bytes: u64,
	pub processes: u32,
	pub working_bytes: u64,
	pub temporary_bytes: u64,
}
pub fn rounded_working_bytes(working_bytes: u64, page_size: u64) -> Option<u64> {
	if page_size == 0 {
		return None;
	}
	let pages = working_bytes
		.checked_add(page_size - 1)?
		.checked_div(page_size)?;
	Some(pages.checked_mul(page_size)?.max(page_size))
}
impl HealthProfile {
	pub fn matches(&self, health: &Value, python: bool) -> bool {
		let limits = &health["probe"]["resources"];
		let working_bytes_valid = health["probe"]["physical_page_size"]
			.as_u64()
			.and_then(|page| rounded_working_bytes(self.working_bytes, page))
			.map_or(limits["working_bytes"] == self.working_bytes, |expected| {
				limits["working_bytes"].as_u64() == Some(expected)
			});
		!(health["protocol"] != "aidash-runner/1"
			|| health["verified"] != true
			|| health["image"] != self.image
			|| health["runtime_class"] != self.runtime_class
			|| (python && health["python_verified"] != true)
			|| limits["cpu"].as_f64() != Some(self.cpu as f64)
			|| limits["memory_bytes"] != self.memory_bytes
			|| limits["swap_bytes"] != 0
			|| limits["processes"] != self.processes
			|| !working_bytes_valid
			|| limits["temporary_bytes"] != self.temporary_bytes)
	}
}
#[cfg(test)]
mod tests;
