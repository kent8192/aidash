use super::{PluginName, ServerName};
use sha2::{Digest, Sha256};

const MAX_ALIAS: usize = 64;
const KEPT_PREFIX: usize = 55;

/// The tool name a model sees: `<plugin>__<server>__<tool>` limited to `[A-Za-z0-9_-]`.
///
/// An alias longer than 64 characters keeps its first 55, then `_` and eight hex digits of a hash
/// over the raw inputs, so inputs that share a long prefix stay distinct.
pub fn model_alias(plugin: &PluginName, server: &ServerName, tool: &str) -> String {
	let alias: String = format!("{plugin}__{server}__{tool}")
		.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
				c
			} else {
				'_'
			}
		})
		.collect();
	if alias.len() <= MAX_ALIAS {
		return alias;
	}
	let mut hasher = Sha256::new();
	hasher.update(plugin.as_str().as_bytes());
	hasher.update([0]);
	hasher.update(server.as_str().as_bytes());
	hasher.update([0]);
	hasher.update(tool.as_bytes());
	let hash = format!("{:x}", hasher.finalize());
	format!("{}_{}", &alias[..KEPT_PREFIX], &hash[..8])
}
