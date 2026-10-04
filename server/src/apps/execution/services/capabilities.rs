//! Tool adapters bind portable execution to native repositories and bootstrap.
use crate::{
	Result, domain::Run, federation::Home, provider::ToolSpec, registry::Entry, store::Store,
};
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone)]
pub struct ToolContext {
	pub home: Home,
	pub store: Store,
	pub run: Run,
}
#[async_trait]
pub trait Tool: Send + Sync {
	fn specification(&self) -> ToolSpec;
	fn replay_safe(&self) -> bool;
	async fn invoke(&self, context: &ToolContext, input: Value, key: &str) -> Result<Value>;
}
pub fn validate_config(value: &Value) -> Result<()> {
	validate_config_in(value, true)
}
pub(crate) fn validate_config_in(value: &Value, local: bool) -> Result<()> {
	crate::bootstrap::registry_validation()
		.validate_tool(value, local)
		.map_err(Into::into)
}

pub struct PluginTool {
	pub entry: Entry,
	pub alias: String,
	pub config: ToolConfig,
	pub client: reqwest::Client,
}
impl PluginTool {
	fn application(&self) -> aidash_application::tools::Plugin {
		aidash_application::tools::Plugin {
			entry: self.entry.clone(),
			alias: self.alias.clone(),
			config: self.config.clone(),
		}
	}
}
#[async_trait]
impl Tool for PluginTool {
	fn specification(&self) -> ToolSpec {
		self.application().specification()
	}
	fn replay_safe(&self) -> bool {
		self.application().replay_safe()
	}
	async fn invoke(&self, ctx: &ToolContext, input: Value, key: &str) -> Result<Value> {
		let operations = crate::apps::execution::repositories::tools::Operations(ctx.clone());
		let context = aidash_application::tools::ToolContext {
			operations: &operations,
			run: &ctx.run,
		};
		self.application()
			.invoke(
				&context,
				&crate::bootstrap::tool_transport(self.client.clone()),
				input,
				key,
			)
			.await
			.map_err(Into::into)
	}
}
struct Builtin(aidash_application::tools::Builtin);
#[async_trait]
impl Tool for Builtin {
	fn specification(&self) -> ToolSpec {
		self.0.specification()
	}
	fn replay_safe(&self) -> bool {
		self.0.replay_safe()
	}
	async fn invoke(&self, ctx: &ToolContext, input: Value, key: &str) -> Result<Value> {
		let operations = crate::apps::execution::repositories::tools::Operations(ctx.clone());
		self.0
			.invoke(
				&aidash_application::tools::ToolContext {
					operations: &operations,
					run: &ctx.run,
				},
				input,
				key,
			)
			.await
			.map_err(Into::into)
	}
}
pub fn builtins() -> BTreeMap<String, Arc<dyn Tool>> {
	aidash_application::tools::builtins()
		.into_iter()
		.map(|(name, tool)| (name, Arc::new(Builtin(tool)) as Arc<dyn Tool>))
		.collect()
}
pub fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
	aidash_application::tools::required(value, key).map_err(Into::into)
}
#[cfg(test)]
use aidash_application::execution::bounded_utf8_end;
pub use aidash_application::tools::plugin_specification;
pub use aidash_domain::tool::ToolConfig;
#[cfg(test)]
#[path = "../tests/services_capabilities_tests.rs"]
mod tests;
