//! Reference bindings pin source digests and require file capability.
use crate::{Error, Result, registry::AgentConfig};

/// The most Reference files one Agent version can bind across its Sources.
pub const MAX_REFERENCE_FILES: usize = 256;
/// The folder an Agent's file tools show a Reference set under, so a Reference
/// file's path never collides with a path the Agent writes.
pub const REFERENCE_SET_ROOT: &str = "references";

/// The suffix a PDF or Excel file's extracted text is shown under, beside it.
pub const EXTRACTION_SUFFIX: &str = ".extracted.txt";

/// A path a Source can give a Reference file. The extraction suffix stays free,
/// so a file's extracted text can never collide with another file's path.
pub fn valid_path(path: &str) -> bool {
	crate::registry::rules::validate_path(path).is_ok() && !path.ends_with(EXTRACTION_SUFFIX)
}

pub fn validate_config(config: &AgentConfig) -> Result<()> {
	let bindings = &config.reference_attachments;
	let mut seen = std::collections::BTreeSet::new();
	let mut paths = std::collections::BTreeSet::new();
	if bindings.len() > MAX_REFERENCE_FILES
		|| !bindings.is_empty() && !config.core_capabilities.files
		|| bindings.iter().any(|r| {
			!seen.insert(r.reference_id)
				|| !paths.insert(r.path.as_str())
				|| !valid_path(&r.path)
				|| r.digest.len() != 64
				|| !r.digest.bytes().all(|b| b.is_ascii_hexdigit())
		}) {
		return Err(Error::Invalid("INVALID_REFERENCE_BINDINGS".into()));
	}
	Ok(())
}

pub mod lifecycle;
