//! Provider implementations dispatch only exact admitted Binding operations.
use crate::{
	federation::{Federation, Home},
	store::Store,
	tool::{PluginTool, Tool, ToolContext},
};
use aidash_application::{
	Error, Result,
	ports::{
		bindings::{BindingAuthority, ProviderCatalog, ProviderSet},
		execution::ExecutionTool,
	},
};
use aidash_domain::{
	Run,
	provider::ToolSpec,
	registry::{
		bindings::{QualifiedRef, ResolvedBinding},
		rules::digest,
	},
	tool::{ToolContract, providers::ToolDescriptor},
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub(crate) struct Providers {
	pub federation: Federation,
	pub home: Home,
}
impl ProviderCatalog for Providers {
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
		let tool: Arc<dyn Tool> = if let Some(transport) = descriptor.transport {
			Arc::new(PluginTool {
				entry: binding.definition.clone(),
				alias: binding
					.alias
					.clone()
					.ok_or_else(|| Error::Invalid("missing tool alias".into()))?,
				config: transport,
				client: self.federation.client.clone(),
			})
		} else {
			let mut implementations = crate::tool::builtins();
			implementations.extend(crate::capabilities::tools::implementations());
			implementations
				.remove(&descriptor.operation)
				.ok_or_else(|| {
					Error::Invalid(format!("PROVIDER_UNAVAILABLE: {}", descriptor.provider))
				})?
		};
		Ok(Arc::new(ProviderTool {
			tool,
			contract,
			store: self.federation.store.clone(),
			home: self.home.clone(),
		}))
	}
}
struct ProviderTool {
	tool: Arc<dyn Tool>,
	contract: ToolContract,
	store: Store,
	home: Home,
}
#[async_trait]
impl ExecutionTool for ProviderTool {
	fn contract(&self) -> ToolContract {
		self.contract.clone()
	}
	fn specification(&self) -> ToolSpec {
		self.tool.specification()
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, run: &Run, input: Value, key: &str) -> Result<Value> {
		self.tool
			.invoke(
				&ToolContext {
					home: self.home.clone(),
					store: self.store.clone(),
					run: run.clone(),
				},
				input,
				key,
			)
			.await
			.map_err(Into::into)
	}
}

/// Unscoped execution is the operator path. Tenant installations still require
/// a retained tenant/subject lease, so this adapter cannot borrow their grants.
pub(crate) struct OperatorAuthority {
	pub federation: Federation,
}
#[async_trait]
impl BindingAuthority for OperatorAuthority {
	async fn refresh(&self, run: &Run) -> Result<()> {
		let snapshot = run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| Error::Invalid("Run has no Binding snapshot".into()))?;
		snapshot.validate()?;
		for pinned in &snapshot.foreign_agents {
			let current: aidash_domain::registry::bindings::ForeignAgentSnapshot = self
				.federation
				.request(
					&pinned.agent.registry_node,
					reqwest::Method::GET,
					&format!(
						"/discover/{}/{}/bindings",
						pinned.agent.id, pinned.agent.version
					),
					None,
				)
				.await?;
			current.validate()?;
			if current != *pinned {
				return Err(Error::Conflict(
					"admitted foreign Agent closure changed".into(),
				));
			}
		}
		Ok(())
	}
	async fn check(&self, _: &Run, binding: &ResolvedBinding) -> Result<()> {
		if binding.identity.registry_node != self.federation.config.node_id
			|| binding.installation.is_some()
		{
			return Err(Error::Forbidden);
		}
		let current = self
			.federation
			.registry
			.get(&binding.identity.id, &binding.identity.version)
			.await?;
		if digest(&serde_json::to_value(current)?) != binding.digest {
			return Err(Error::Conflict(
				"admitted Binding definition changed".into(),
			));
		}
		Ok(())
	}
}
