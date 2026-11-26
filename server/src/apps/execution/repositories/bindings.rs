//! Provider implementations dispatch only exact admitted Binding operations.
use crate::{
	federation::{Federation, Home},
	store::Store,
	tool::{PluginTool, Tool, ToolContext},
};
use aidash_application::{
	Error, Result,
	ports::{
		bindings::{ProviderCatalog, ProviderSet},
		execution::ExecutionTool,
	},
	registry::bindings::execution::{bound_specification, validate_bound_input},
};
use aidash_domain::{
	Run,
	provider::ToolSpec,
	registry::bindings::{QualifiedRef, ResolvedBinding},
	tool::{
		ToolContract,
		concurrency::{ConcurrentCall, ReadCeilings, batchable, core_call},
		providers::ToolDescriptor,
	},
};
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub(crate) struct Providers {
	pub federation: Federation,
	pub home: Home,
	/// The Run's Tenant scope, which every bound tool works in.
	pub scope: aidash_application::tenancy::TenancyScope,
}
impl ProviderCatalog for Providers {
	fn decision_implementation(
		&self,
		config: &aidash_domain::decision::DeciderConfig,
	) -> Result<String> {
		crate::bootstrap::registry_validation_for(&self.federation.store)
			.decision_implementation(config)
	}
	fn contract(
		&self,
		descriptor: &ToolDescriptor,
		identity: &QualifiedRef,
	) -> Result<ToolContract> {
		crate::bootstrap::registry_validation().contract(descriptor, identity)
	}
	fn implementation(&self, descriptor: &ToolDescriptor) -> Result<String> {
		crate::bootstrap::registry_validation().implementation(descriptor)
	}
}
#[async_trait]
impl ProviderSet for Providers {
	async fn bind(&self, _: &Run, binding: &ResolvedBinding) -> Result<Arc<dyn ExecutionTool>> {
		let descriptor: ToolDescriptor = serde_json::from_value(binding.definition.config.clone())?;
		self.implementation(&descriptor)?;
		let contract = self.contract(&descriptor, &binding.identity)?;
		let connection_scopes = match &descriptor.transport {
			Some(aidash_domain::tool::ToolConfig::Mcp { scopes, .. }) => {
				scopes.clone().unwrap_or_default()
			}
			_ => Vec::new(),
		};
		let operation = descriptor
			.transport
			.is_none()
			.then(|| descriptor.operation.clone());
		Ok(Arc::new(ProviderTool {
			tool: self.tool(binding, descriptor, &implementations())?,
			description_note: None,
			pinned: None,
			contract,
			operation,
			connection_scopes,
			store: self.federation.store.clone(),
			home: self.home.clone(),
			scope: self.scope.clone(),
		}))
	}
}
fn implementations() -> BTreeMap<String, Arc<dyn Tool>> {
	let mut implementations = crate::tool::builtins();
	implementations.extend(crate::capabilities::tools::implementations());
	implementations
}
impl Providers {
	/// Coordination tools are intrinsic to a Subnode, independent of a member's Bindings.
	pub(crate) fn coordination_tool(
		&self,
		run: &Run,
		name: &str,
	) -> Result<Arc<dyn ExecutionTool>> {
		let pinned = run.context.binding_snapshot.as_ref().and_then(|snapshot| {
			snapshot.bindings.iter().find(|binding| {
				binding.excluded_reason.is_none()
					&& binding.alias.as_deref() == Some(name)
					&& binding.definition.config["operation"] == name
					&& binding.definition.config["provider"]
						.as_str()
						.is_some_and(|provider| provider.starts_with("core."))
			})
		});
		self.intrinsic(name, None, pinned.cloned())
	}

	pub(crate) fn intrinsic_tool(
		&self,
		name: &str,
		description_note: Option<String>,
	) -> Result<Arc<dyn ExecutionTool>> {
		self.intrinsic(name, description_note, None)
	}

	fn intrinsic(
		&self,
		name: &str,
		description_note: Option<String>,
		pinned: Option<ResolvedBinding>,
	) -> Result<Arc<dyn ExecutionTool>> {
		let tool = implementations()
			.remove(name)
			.ok_or_else(|| Error::Invalid(format!("missing coordination tool: {name}")))?;
		let mut contract = tool.contract();
		contract.authorization.flag = None;
		contract.authorization.core = None;
		Ok(Arc::new(ProviderTool {
			tool,
			description_note,
			pinned,
			contract,
			operation: Some(name.into()),
			connection_scopes: Vec::new(),
			store: self.federation.store.clone(),
			home: self.home.clone(),
			scope: self.scope.clone(),
		}))
	}
	fn tool(
		&self,
		binding: &ResolvedBinding,
		descriptor: ToolDescriptor,
		implementations: &BTreeMap<String, Arc<dyn Tool>>,
	) -> Result<Arc<dyn Tool>> {
		if let Some(research) =
			crate::apps::execution::services::research::ResearchTool::bind(&descriptor)
		{
			return Ok(Arc::new(research));
		}
		Ok(if let Some(transport) = descriptor.transport {
			Arc::new(PluginTool {
				entry: binding.definition.clone(),
				alias: binding
					.alias
					.clone()
					.ok_or_else(|| Error::Invalid("missing tool alias".into()))?,
				config: transport,
				transport: crate::bootstrap::tool_transport(&self.federation.store),
			})
		} else {
			implementations
				.get(&descriptor.operation)
				.cloned()
				.ok_or_else(|| {
					Error::Invalid(format!("PROVIDER_UNAVAILABLE: {}", descriptor.provider))
				})?
		})
	}
	/// Tool specifications of a Run's aliased Bindings exactly as `bind` and
	/// `BoundTool` advertise them, without binding authority checks.
	pub(crate) fn specifications(&self, run: &Run) -> Result<BTreeMap<String, ToolSpec>> {
		let snapshot = run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| Error::Invalid("Run has no admitted Binding snapshot".into()))?;
		let implementations = implementations();
		aidash_application::registry::bindings::execution::bound_specifications(
			snapshot,
			|binding, descriptor| {
				self.implementation(&descriptor)?;
				Ok(self
					.tool(binding, descriptor, &implementations)?
					.specification())
			},
		)
	}
}
struct ProviderTool {
	tool: Arc<dyn Tool>,
	description_note: Option<String>,
	/// The declaration of a coordination alias that the Agent also bound.
	pinned: Option<ResolvedBinding>,
	contract: ToolContract,
	/// The core provider operation; integrations carry a transport instead.
	operation: Option<String>,
	connection_scopes: Vec<String>,
	store: Store,
	home: Home,
	scope: aidash_application::tenancy::TenancyScope,
}
#[async_trait]
impl ExecutionTool for ProviderTool {
	fn connection_scopes(&self) -> Vec<String> {
		self.connection_scopes.clone()
	}
	fn contract(&self) -> ToolContract {
		self.contract.clone()
	}
	fn specification(&self) -> ToolSpec {
		let mut spec = self.tool.specification();
		if let Some(pinned) = &self.pinned {
			spec = bound_specification(pinned, spec);
		}
		if let Some(note) = &self.description_note {
			spec.description.push_str(note);
		}
		spec
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	fn validate_input(&self, input: &Value) -> Result<()> {
		if let Some(pinned) = &self.pinned {
			validate_bound_input(pinned, input)?;
		}
		Ok(())
	}
	async fn invoke(&self, run: &Run, input: Value, key: &str) -> Result<Value> {
		self.validate_input(&input)?;
		self.tool
			.invoke(
				&ToolContext {
					home: self.home.clone(),
					store: self.store.clone(),
					run: run.clone(),
					scope: self.scope.clone(),
				},
				input,
				key,
			)
			.await
			.map_err(Into::into)
	}
	fn concurrent_call(&self, input: &Value) -> Option<ConcurrentCall> {
		if !batchable(&self.contract.behavior) {
			return None;
		}
		let limits = &self.store.capabilities.0.limits;
		core_call(
			self.operation.as_deref()?,
			input,
			ReadCeilings {
				read_bytes: limits.read_bytes,
				search_bytes: limits.search_bytes,
			},
		)
	}
}
