//! Native, identity-bound ports for portable tool execution.
use aidash_application::{Result, ports::tools::ToolOperations};
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, Task,
	exposure::DirectSkill,
	provider::ToolSpec,
	registry::{EntityRef, Search, SkillFile},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;
pub(crate) struct Operations(pub(crate) crate::tool::ToolContext);
fn operation_error(error: crate::Error) -> aidash_application::Error {
	match error {
		crate::Error::NotFound(_) | crate::Error::Conflict(_) | crate::Error::Forbidden => {
			aidash_application::Error::Invalid(error.to_string())
		}
		other => other.into(),
	}
}
fn observed_media_type(run: &aidash_domain::Run, id: &str) -> Option<String> {
	run.context.history.iter().rev().find_map(|entry| {
		let aidash_domain::context::ContextEvent::Tool { call, result } = &entry.event else {
			return None;
		};
		if call.name == "workspace_read" && result["metadata"]["id"].as_str() == Some(id) {
			return result["metadata"]["media_type"].as_str().map(str::to_owned);
		}
		if call.name == "workspace_read"
			&& result["kind"] == "message"
			&& result["offset"] == 0
			&& result["next_offset"].is_null()
		{
			let record: Value = serde_json::from_str(result["content"].as_str()?).ok()?;
			return record["attachments"]
				.as_array()?
				.iter()
				.find_map(|attachment| {
					(attachment["id"].as_str() == Some(id))
						.then(|| attachment["media_type"].as_str().map(str::to_owned))
						.flatten()
				});
		}
		if call.name != "workspace_observe" {
			return None;
		}
		result["messages"].as_array()?.iter().find_map(|message| {
			message["attachments"]
				.as_array()?
				.iter()
				.find_map(|attachment| {
					(attachment["id"].as_str() == Some(id))
						.then(|| attachment["media_type"].as_str().map(str::to_owned))
						.flatten()
				})
		})
	})
}

pub(crate) fn run_model(run: &aidash_domain::Run) -> Result<aidash_domain::model::ModelConfig> {
	let snapshot = run
		.context
		.binding_snapshot
		.as_ref()
		.ok_or(aidash_application::Error::Forbidden)?;
	let agent = aidash_domain::registry::AgentConfig::from_snapshot(snapshot)?;
	let model = snapshot
		.definitions
		.iter()
		.find(|entry| {
			entry.identity.local() == agent.model
				&& entry.identity.registry_deployment == snapshot.agent.registry_deployment
		})
		.ok_or(aidash_application::Error::Forbidden)?;
	Ok(serde_json::from_value(model.definition.config.clone())?)
}
#[async_trait]
impl ToolOperations for Operations {
	async fn registered_skills(&self) -> Result<Vec<EntityRef>> {
		let snapshot = self
			.0
			.run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| {
				aidash_application::Error::Invalid("Run has no Binding snapshot".into())
			})?;
		Ok(aidash_domain::registry::AgentConfig::from_snapshot(snapshot)?.skills)
	}
	async fn skill_files(&self, reference: &EntityRef) -> Result<Vec<SkillFile>> {
		let snapshot = self
			.0
			.run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| {
				aidash_application::Error::Invalid("Run has no Binding snapshot".into())
			})?;
		let entry = snapshot
			.bindings
			.iter()
			.find(|b| b.definition.kind == "skill" && b.identity.local() == *reference)
			.ok_or(aidash_application::Error::Forbidden)?;
		crate::registry::skill_files(&entry.definition).map_err(Into::into)
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
	async fn message_with_files(&self, key: &str, content: &str, files: &[String]) -> Result<()> {
		if files.is_empty() {
			return self.message(key, content).await;
		}
		if !self.0.run.is_local(self.0.store.deployment.as_str()) {
			return Err(aidash_application::Error::Forbidden);
		}
		let authority = self
			.0
			.home
			.authority
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		authority
			.message_with_files(&self.0.store, &self.0.run, key, content, files)
			.await
			.map_err(operation_error)
	}
	async fn attachment(&self, id: &str, model_input: bool) -> Result<Value> {
		let model = if model_input {
			let model = run_model(&self.0.run)?;
			let observed = observed_media_type(&self.0.run, id);
			if let Some(media_type) = observed {
				aidash_application::execution::media::thread_attachment_wav(&model, &media_type)?;
			}
			Some(model)
		} else {
			None
		};
		let record = self.0.home.read_record("attachment", id).await?;
		let metadata = record["attachment"].clone();
		if !model_input {
			return Ok(json!({"status":"completed", "metadata":metadata}));
		}
		let media_type = metadata["media_type"]
			.as_str()
			.ok_or(aidash_application::Error::Forbidden)?;
		let model = model.ok_or(aidash_application::Error::Forbidden)?;
		let wav = aidash_application::execution::media::thread_attachment_wav(&model, media_type)?;
		Ok(
			json!({"status":"completed", "metadata":metadata, "selection":{
				"attachment_id":id, "message_id":record["message_id"], "expected_digest":record["sha256"], "wav":wav
			}}),
		)
	}
	async fn attachment_copy(&self, input: Value) -> Result<Value> {
		if !self.0.run.is_local(self.0.store.deployment.as_str()) {
			return Err(aidash_application::Error::Forbidden);
		}
		let authority = self
			.0
			.home
			.authority
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		authority
			.copy_thread_attachment(&self.0.store, &self.0.run, input)
			.await
			.map_err(operation_error)
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

	async fn memory_mutate(
		&self,
		key: &str,
		changes: &[aidash_domain::memory::Change],
	) -> Result<Vec<aidash_domain::memory::Unit>> {
		let ctx = &self.0;
		if !ctx.run.is_local(ctx.store.deployment.as_str()) {
			return Err(aidash_application::Error::Forbidden);
		}
		if let Some(authority) = &ctx.home.authority {
			authority
				.memory_mutate(&ctx.store, &ctx.run, key, changes)
				.await
				.map_err(Into::into)
		} else if ctx.home.local() {
			let mut lease = crate::semantic::service::Lease::within(&ctx.store, &ctx.scope).await?;
			let result = crate::semantic::native_memory::run_mutate(
				&ctx.store, &mut lease, &ctx.run, key, changes,
			)
			.await;
			lease.finish(result).await.map_err(Into::into)
		} else {
			Err(aidash_application::Error::Forbidden)
		}
	}
	async fn memory_recall(
		&self,
		key: &str,
		query: &aidash_domain::memory::RecallQuery,
		reflect: bool,
	) -> Result<Value> {
		let ctx = &self.0;
		if let Some(authority) = &ctx.home.authority {
			return Ok(json!(
				authority
					.memory_recall(&ctx.store, &ctx.run, key, query.clone(), reflect)
					.await?
			));
		}
		let mut lease = crate::semantic::service::Lease::within(&ctx.store, &ctx.scope).await?;
		let result = crate::semantic::native_memory::run_recall(
			&ctx.store,
			&mut lease,
			&ctx.run,
			key,
			query.clone(),
			reflect,
		)
		.await;
		Ok(json!(lease.finish(result).await?))
	}
	async fn human_request(&self, kind: &str, prompt: &str, key: &str) -> Result<HumanRequest> {
		self.0
			.home
			.human_request(kind, prompt, key)
			.await
			.map_err(Into::into)
	}
	async fn binding_specifications(&self) -> Result<BTreeMap<String, ToolSpec>> {
		super::bindings::Providers {
			federation: self.0.home.federation.clone(),
			home: self.0.home.clone(),
			scope: self.0.scope.clone(),
		}
		.specifications(&self.0.run)
	}
	async fn direct_skills(&self) -> Result<Vec<DirectSkill>> {
		match &self.0.home.authority {
			Some(authority) => authority
				.direct_skills(&self.0.store, &self.0.run)
				.await
				.map_err(Into::into),
			None => Ok(vec![]),
		}
	}
	async fn direct_skill_file(
		&self,
		skill_id: Uuid,
		digest: &str,
		path: &str,
	) -> Result<Option<Vec<u8>>> {
		self.0
			.home
			.authority
			.as_ref()
			.ok_or_else(|| aidash_application::Error::NotFound("skill unavailable".into()))?
			.direct_skill_file(&self.0.store, &self.0.run, skill_id, digest, path)
			.await
			.map_err(Into::into)
	}
	fn skill_read_bytes(&self) -> Result<usize> {
		Ok(self.0.store.capabilities.0.limits.read_bytes)
	}
	async fn tenant_secret(&self, name: &str, endpoint: &str) -> Result<secrecy::SecretString> {
		// The value is read from this node's own Secret Store, in the Run's
		// Tenant: under Federation it never leaves the owning Tenant's Node.
		let unavailable = || {
			aidash_application::Error::Invalid(aidash_domain::tenant_secrets::UNAVAILABLE.into())
		};
		let (Some(service), Some(tenant)) =
			(&self.0.store.tenant_secrets, self.0.scope.tenant_name())
		else {
			return Err(unavailable());
		};
		service.resolve(tenant.as_str(), name, endpoint).await
	}
}
