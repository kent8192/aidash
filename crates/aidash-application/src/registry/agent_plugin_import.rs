//! Fetch an Agent Plugin directory through the injected source and validate it.
use crate::{
	Result,
	ports::registry::{AgentPluginSource, PluginFetch, PluginImportRequest},
};
use aidash_domain::agent_plugin::{self, LoadedPlugin, PluginFile};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginImport {
	/// The repository holds several plugins; these are their roots.
	Choose(Vec<String>),
	Loaded {
		entitlement_expires_at: Option<chrono::DateTime<chrono::Utc>>,
		/// `https://github.com/<owner>/<repo>/tree/<commit sha>[/<root>]`.
		source: String,
		/// The plugin directory relative to the repository root; empty for the root.
		root: String,
		/// Paths the specification forbids access to, left out of `files`.
		denied: Vec<String>,
		plugin: Box<LoadedPlugin>,
		files: Vec<PluginFile>,
	},
}

#[derive(Clone)]
pub struct AgentPluginImport {
	source: Arc<dyn AgentPluginSource>,
}

impl AgentPluginImport {
	pub fn new(source: Arc<dyn AgentPluginSource>) -> Self {
		Self { source }
	}

	/// A `Rejection` of the fetched directory becomes `Error::Invalid` with the domain's code.
	pub async fn import(&self, request: PluginImportRequest) -> Result<PluginImport> {
		match self.source.fetch(request).await? {
			PluginFetch::Choose(roots) => Ok(PluginImport::Choose(roots)),
			PluginFetch::Fetched(fetched) => {
				let plugin =
					agent_plugin::load(&fetched.files).map_err(aidash_domain::Error::from)?;
				Ok(PluginImport::Loaded {
					source: fetched.source,
					entitlement_expires_at: fetched.entitlement_expires_at,
					root: fetched.root,
					denied: fetched.denied,
					plugin: Box::new(plugin),
					files: fetched.files,
				})
			}
		}
	}
}

#[cfg(test)]
mod tests;
