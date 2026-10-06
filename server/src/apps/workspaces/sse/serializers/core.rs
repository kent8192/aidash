use serde::Serialize;
// Serializable core contracts.

/// Process-local diagnostics, also exported through the existing private metrics.
#[derive(Debug, Serialize)]
pub struct Snapshot {
	pub event_queries: u64,
	pub query_causes: [u64; 6],
	pub notifications: u64,
	pub rejected_notifications: u64,
	pub authority_checks: u64,
	pub gate_checks: u64,
	pub visibility_waiters: u64,
	pub frames: u64,
	pub backpressure_disconnects: u64,
	pub transport_ready: bool,
	pub registered_scopes: usize,
}
