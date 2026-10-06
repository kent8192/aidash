//! Reconstruct dispatch from durable Bindings while checking live Node authority.
use crate::{
	Error, Result,
	ports::{bindings::*, execution::*},
};
use aidash_domain::{
	Run,
	provider::ToolSpec,
	registry::{bindings::*, rules::digest},
	tool::ToolContract,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct PinnedResolver {
	pub providers: Arc<dyn ProviderSet>,
	pub authority: Arc<dyn BindingAuthority>,
}
#[async_trait]
impl BindingResolver for PinnedResolver {
	async fn tools(&self, run: &Run) -> Result<Tools> {
		let snapshot = run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| Error::Invalid("Run has no admitted Binding snapshot".into()))?;
		snapshot.validate()?;
		if snapshot.agent.id != run.agent_id || snapshot.agent.version != run.agent_version {
			return Err(Error::Conflict(
				"Run Agent differs from its Binding snapshot".into(),
			));
		}
		let snapshot_digest = digest(&serde_json::to_value(snapshot)?);
		let mut tools = Tools::new();
		for binding in &snapshot.bindings {
			if binding.excluded_reason.is_some() {
				continue;
			}
			self.authority.check(run, binding).await?;
			let Some(alias) = &binding.alias else {
				continue;
			};
			let contract = super::recheck_provider(self.providers.as_ref(), binding)?;
			let inner = self.providers.bind(run, binding).await?;
			if inner.contract() != contract {
				return Err(Error::Conflict(
					"Provider dispatch differs from its admitted operation contract".into(),
				));
			}
			tools.insert(
				alias.clone(),
				Arc::new(BoundTool {
					inner,
					binding: binding.clone(),
					contract,
					run: run.id,
					snapshot_digest: snapshot_digest.clone(),
					providers: self.providers.clone(),
					authority: self.authority.clone(),
				}) as Arc<dyn ExecutionTool>,
			);
		}
		Ok(tools)
	}
}
struct BoundTool {
	inner: Arc<dyn ExecutionTool>,
	binding: ResolvedBinding,
	contract: ToolContract,
	run: uuid::Uuid,
	snapshot_digest: String,
	providers: Arc<dyn ProviderSet>,
	authority: Arc<dyn BindingAuthority>,
}
#[async_trait]
impl ExecutionTool for BoundTool {
	fn specification(&self) -> ToolSpec {
		let mut specification = self.inner.specification();
		specification.name = self.binding.alias.clone().expect("resolved tool alias");
		specification.parameters = self.binding.definition.schema.clone();
		if let Some(description) = self.binding.definition.description.get("en") {
			specification.description = description.clone();
		}
		specification
	}
	fn contract(&self) -> ToolContract {
		self.contract.clone()
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, run: &Run, mut input: Value, key: &str) -> Result<Value> {
		if run.id != self.run
			|| run
				.context
				.binding_snapshot
				.as_ref()
				.is_none_or(|snapshot| {
					serde_json::to_value(snapshot)
						.map_or(true, |value| digest(&value) != self.snapshot_digest)
				}) {
			return Err(Error::Conflict(
				"tool dispatch belongs to a different admitted Run".into(),
			));
		}
		super::recheck_provider(self.providers.as_ref(), &self.binding)?;
		self.authority.check(run, &self.binding).await?;
		self.binding.narrow.apply(&mut input)?;
		self.inner.invoke(run, input, key).await
	}
}
