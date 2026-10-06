//! HTTP/worker error conversion around the shared bounded observation rules.
pub use aidash_domain::context::observation::{fit_projection, normalize_history, project};
pub fn chunk_record(
	value: serde_json::Value,
	kind: &str,
	id: &str,
	offset: usize,
	max_chars: usize,
) -> crate::Result<serde_json::Value> {
	aidash_domain::context::observation::chunk_record(value, kind, id, offset, max_chars)
		.map_err(Into::into)
}
