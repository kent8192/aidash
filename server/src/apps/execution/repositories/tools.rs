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
#[async_trait]
impl ToolOperations for Operations {
	async fn connection(
		&self,
		resource: &str,
		_call: aidash_application::tenant_secrets::SecretCall,
		key: &str,
	) -> Result<aidash_application::tenant_secrets::CallSecret> {
		super::mcp_connections::connection(&self.0.store, &self.0.run, &self.0.scope, resource, key)
			.await
	}
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
	async fn tenant_secret(
		&self,
		name: &str,
		endpoint: &str,
		call: aidash_application::tenant_secrets::SecretCall,
		key: &str,
	) -> Result<aidash_application::tenant_secrets::CallSecret> {
		self.tenant_secret_for(
			name,
			endpoint,
			aidash_domain::tool::SecretOwner::Any,
			call,
			key,
		)
		.await
	}
	async fn tenant_secret_for(
		&self,
		name: &str,
		endpoint: &str,
		owner: aidash_domain::tool::SecretOwner,
		call: aidash_application::tenant_secrets::SecretCall,
		key: &str,
	) -> Result<aidash_application::tenant_secrets::CallSecret> {
		// The Tenant Secret is resolved in the Run's own Tenant: under Federation
		// it never leaves the owning Tenant's Node. A self-hosted Store reads the
		// value; a Cloud or Dedicated Store mints a broker route instead.
		let unavailable = || {
			if owner == aidash_domain::tool::SecretOwner::Personal {
				aidash_application::tenant_secrets::personal_required(name)
			} else {
				aidash_application::Error::Invalid(
					aidash_domain::tenant_secrets::UNAVAILABLE.into(),
				)
			}
		};
		let (Some(service), Some(tenant)) =
			(&self.0.store.tenant_secrets, self.0.scope.tenant_name())
		else {
			return Err(unavailable());
		};
		let person = crate::apps::identity::repositories::oauth_connections::owner_of_run(
			&self.0.store,
			&self.0.run,
		)
		.await?;
		if person
			.as_ref()
			.is_some_and(|person| person.tenant != tenant.as_str())
		{
			return Err(aidash_application::Error::Forbidden);
		}
		let secret = service
			.call_secret_for(
				tenant.as_str(),
				person.as_ref().map(|person| person.subject.as_str()),
				name,
				owner,
				endpoint,
				call,
			)
			.await?;
		// A call that carried a Tenant Secret records which one, its pinned
		// version and its host. The value is never recorded.
		self.0
			.store
			.invocation_secret(
				&self.0.scope,
				key,
				secret.record().expect("Tenant Secret record"),
			)
			.await?;
		Ok(secret)
	}
}
