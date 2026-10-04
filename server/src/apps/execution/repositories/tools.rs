//! Native, identity-bound ports for portable tool execution.
use aidash_application::{Result, ports::tools::ToolOperations};
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, Task,
	registry::{EntityRef, Search, SkillFile},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Operations(pub(crate) crate::tool::ToolContext);
#[async_trait]
impl ToolOperations for Operations {
	async fn registered_skills(&self) -> Result<Vec<EntityRef>> {
		let registry =
			crate::registry::Registry::new(self.0.store.pool.clone(), &self.0.store.node_id)?;
		let run = &self.0.run;
		let agent = registry
			.get_for_run(run, &run.agent_id, &run.agent_version)
			.await?;
		let config: crate::registry::AgentConfig = serde_json::from_value(agent.config)?;
		Ok(config.skills)
	}
	async fn skill_files(&self, reference: &EntityRef) -> Result<Vec<SkillFile>> {
		let registry =
			crate::registry::Registry::new(self.0.store.pool.clone(), &self.0.store.node_id)?;
		let entry = registry
			.get_for_run(&self.0.run, &reference.id, &reference.version)
			.await?;
		crate::registry::skill_files(&entry).map_err(Into::into)
	}
	async fn discover(&self, input: &Search) -> Result<Value> {
		Ok(json!(self.0.home.discover(input).await?))
	}
	async fn create_task(&self, key: &str, input: &NewTask) -> Result<Task> {
		self.0
			.home
			.create_task(key, input)
			.await
			.map_err(Into::into)
	}
	async fn assign(&self, id: Uuid, policy: &str, reason: &str) -> Result<Value> {
		Ok(json!(self.0.home.assign(id, policy, reason).await?))
	}
	async fn delegate_with_key(
		&self,
		key: &str,
		id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Value> {
		Ok(json!(
			self.0.home.delegate_with_key(key, id, node, agent).await?
		))
	}
	async fn artifact(&self, key: &str, input: &ArtifactInput) -> Result<Artifact> {
		self.0.home.artifact(key, input).await.map_err(Into::into)
	}
	async fn message(&self, key: &str, content: &str) -> Result<()> {
		self.0.home.message(key, content).await.map_err(Into::into)
	}
	async fn observation(&self, offset: usize, limit: usize) -> Result<Value> {
		self.0
			.home
			.observation(offset, limit)
			.await
			.map_err(Into::into)
	}
	async fn read_record_chunk(
		&self,
		kind: &str,
		id: &str,
		offset: usize,
		maximum: usize,
	) -> Result<Value> {
		self.0
			.home
			.read_record_chunk(kind, id, offset, maximum)
			.await
			.map_err(Into::into)
	}

	async fn remember(&self, input: &Value) -> Result<()> {
		let ctx = &self.0;
		if let Some(authority) = &ctx.home.authority {
			authority.remember(&ctx.store, &ctx.run, input).await?;
		} else if ctx.home.local() {
			let mut lease = crate::semantic::service::Lease::begin(
				&ctx.store,
				&crate::authorization::identity::Actor::Operator,
			)
			.await?;
			let result =
				crate::semantic::service::remember_in(&ctx.store, &mut lease, &ctx.run, input)
					.await;
			lease.finish(result).await?;
		} else {
			ctx.store.remember(&ctx.run, input).await?;
		}
		Ok(())
	}
	async fn human_request(&self, kind: &str, prompt: &str, key: &str) -> Result<HumanRequest> {
		self.0
			.store
			.human_request(&self.0.run, kind, prompt, key)
			.await
			.map_err(Into::into)
	}
}
