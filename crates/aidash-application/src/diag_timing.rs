//! Temporary #321 diagnostics: per-phase wall time of scoped federation calls.
use std::time::Instant;
pub struct PhaseTimer {
	label: &'static str,
	start: Instant,
	last: Instant,
	phases: Vec<(&'static str, u128)>,
}
impl PhaseTimer {
	pub fn new(label: &'static str) -> Self {
		let now = Instant::now();
		Self {
			label,
			start: now,
			last: now,
			phases: vec![],
		}
	}
	pub fn mark(&mut self, phase: &'static str) {
		let now = Instant::now();
		self.phases
			.push((phase, now.duration_since(self.last).as_micros()));
		self.last = now;
	}
}
impl Drop for PhaseTimer {
	fn drop(&mut self) {
		let phases = self
			.phases
			.iter()
			.map(|(name, us)| format!("{name}={}", us / 1000))
			.collect::<Vec<_>>()
			.join(" ");
		tracing::info!(
			target: "aidash_diag",
			label = self.label,
			total_ms = self.start.elapsed().as_millis() as u64,
			phases = %phases,
			"phase timing"
		);
	}
}
