//! Reference bindings pin source digests and require file capability.
use crate::{Error, Result, registry::AgentConfig};

pub fn validate_config(config: &AgentConfig) -> Result<()> {
	let bindings = &config.reference_attachments;
	let mut seen = std::collections::BTreeSet::new();
	if bindings.len() > 8
		|| !bindings.is_empty() && !config.core_capabilities.files
		|| bindings.iter().any(|r| {
			!seen.insert(r.reference_id)
				|| r.digest.len() != 64
				|| !r.digest.bytes().all(|b| b.is_ascii_hexdigit())
		}) {
		return Err(Error::Invalid("INVALID_REFERENCE_BINDINGS".into()));
	}
	Ok(())
}

pub mod lifecycle;
