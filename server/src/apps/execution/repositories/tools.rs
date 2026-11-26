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
		if aidash_domain::tool::thread_tools::is(&call.name, "thread_read")
			&& result["metadata"]["id"].as_str() == Some(id)
		{
			return result["metadata"]["media_type"].as_str().map(str::to_owned);
		}
		if aidash_domain::tool::thread_tools::is(&call.name, "thread_read")
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
		if !aidash_domain::tool::thread_tools::is(&call.name, "thread_observe") {
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
			Some(entry.identity.local()) == agent.model
				&& entry.identity.registry_deployment == snapshot.agent.registry_deployment
		})
		.ok_or(aidash_application::Error::Forbidden)?;
	Ok(aidash_domain::model::ModelConfig::from_stored(
		model.definition.config.clone(),
	)?)
}
#[async_trait]
impl ToolOperations for Operations {
	async fn journal_query(
		&self,
		code: &str,
		target_run: Option<Uuid>,
		range: Option<(u64, u64)>,
		key: &str,
	) -> Result<Value> {
		super::journal_query::execute(
			&self.0.store,
			&self.0.run,
			&self.0.scope,
			code,
			target_run,
			range,
			key,
		)
		.await
	}
	async fn journal(
		&self,
		target_run: Option<Uuid>,
		range: Option<(u64, u64)>,
	) -> Result<Vec<aidash_domain::context::journal_export::JournalLine>> {
		use aidash_application::ports::journal_export::JournalSource;
		self.0
			.store
			.export(
				&self.0.scope,
				self.0.run.id,
				target_run.unwrap_or(self.0.run.id),
				range,
			)
			.await
	}
	async fn subnode_command(
		&self,
		key: &str,
		command: &aidash_application::subnodes::Command,
	) -> Result<Value> {
		self.0
			.home
			.subnode_command(key, command)
			.await
			.map_err(Into::into)
	}
	async fn subnode_create(
		&self,
		key: &str,
		input: &aidash_application::subnodes::Create,
	) -> Result<aidash_domain::subnodes::Subnode> {
		self.0
			.home
			.subnode_create(key, input)
			.await
			.map_err(Into::into)
	}
	async fn connection_call(
		&self,
		call: &aidash_domain::identity::commands::connections::ConnectionCall,
	) -> Result<Option<Value>> {
		if self.0.home.local() {
			return Ok(None);
		}
		let value: Value = self
			.0
			.home
			.command(
				aidash_domain::identity::commands::connections::CALL,
				serde_json::to_value(call)?,
			)
			.await?;
		aidash_application::tools::connection_outcome(value, false)
	}

	async fn book_trigger(&self, trigger: Uuid) -> Result<Value> {
		crate::apps::workspaces::services::duty_events::book(&self.0.store, &self.0.run, trigger)
			.await
			.map_err(Into::into)
	}

	async fn duty(&self, key: &str, operation: &str, input: Value) -> Result<Value> {
		if operation == "duty_notify" {
			// An event leaves for another Node's Duty (#1013); it needs the
			// Federation transport, not a Duty of this Node.
			return crate::apps::workspaces::services::duty_events::notify(
				&self.0.home.federation,
				&self.0.run,
				input,
			)
			.await
			.map_err(Into::into);
		}
		crate::apps::workspaces::services::duties::agent_tool(
			&self.0.home.federation,
			&self.0.run,
			key,
			operation,
			input,
		)
		.await
		.map_err(Into::into)
	}

	async fn node(&self, key: &str, operation: &str, input: Value) -> Result<Value> {
		crate::apps::identity::services::nodes::agent_tool(
			&self.0.home.federation,
			&self.0.run,
			key,
			operation,
			input,
		)
		.await
		.map_err(operation_error)
	}

	async fn connection_person(&self) -> Result<bool> {
		// A Run executing for another Home has no authority here to look the chain root up;
		// the Home holds it and refuses a root that is not a person (ADR 0139).
		if !self.0.run.is_local(self.0.store.deployment.as_str()) {
			return Ok(true);
		}
		Ok(
			crate::apps::identity::repositories::oauth_connections::owner_of_run(
				&self.0.store,
				&self.0.run,
			)
			.await?
			.is_some(),
		)
	}
	async fn connection(
		&self,
		resource: &str,
		call: aidash_application::tenant_secrets::SecretCall,
		key: &str,
	) -> Result<aidash_application::tenant_secrets::CallSecret> {
		super::mcp_connections::connection(
			&self.0.store,
			&self.0.run,
			&self.0.scope,
			resource,
			call,
			key,
		)
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
		// A model chooses an Agent by its card; the whole definition stays with people.
		Ok(json!(self.0.home.discover_cards(input).await?))
	}
	async fn create_task(&self, key: &str, input: &NewTask) -> Result<Task> {
		self.0
			.home
			.create_task(key, input)
			.await
			.map_err(Into::into)
	}
	async fn assign(
		&self,
		id: Uuid,
		policy: &str,
		reason: &str,
		draft: Option<&aidash_domain::generation::policy::Draft>,
	) -> Result<Value> {
		if !self.0.run.is_local(self.0.store.deployment.as_str()) {
			// The Home assigns under its own Generation policy, to its own Agents only.
			// Its scoped `task_assign` carries no free-form draft (#1012), so a Run
			// executing for another Home cannot write one.
			if draft.is_some() {
				return Err(aidash_application::Error::Invalid(
					"a Run executing for another Home cannot draft a generated agent".into(),
				));
			}
			return self
				.0
				.home
				.forward_assign(id, policy, reason)
				.await
				.map_err(Into::into);
		}
		Ok(json!(self.0.home.assign(id, policy, reason, draft).await?))
	}
	async fn delegate_with_key(
		&self,
		key: &str,
		id: Uuid,
		node: &str,
		agent: &EntityRef,
		selection: Option<&aidash_domain::TaskModel>,
	) -> Result<Value> {
		Ok(json!(
			self.0
				.home
				.delegate_selected(key, id, node, agent, selection)
				.await?
		))
	}
	async fn artifact(&self, key: &str, input: &ArtifactInput) -> Result<Artifact> {
		self.0.home.artifact(key, input).await.map_err(Into::into)
	}
	async fn message(&self, key: &str, content: &str) -> Result<()> {
		self.0.home.message(key, content).await.map_err(Into::into)
	}
	async fn message_with_files(
		&self,
		key: &str,
		content: &str,
		files: &[String],
		descriptions: &std::collections::BTreeMap<String, String>,
	) -> Result<()> {
		if files.is_empty() {
			return self.message(key, content).await;
		}
		if !self.0.run.is_local(self.0.store.deployment.as_str()) {
			// The Home takes the bytes as Thread Attachments of the message it posts for the
			// Agent (ADR 0139, decision 20).
			let authority = self
				.0
				.home
				.tool_authority()
				.ok_or(aidash_application::Error::Forbidden)?;
			return authority
				.message_files_to_home(
					&self.0.store,
					&self.0.run,
					&self.0.home,
					key,
					content,
					files,
					descriptions,
				)
				.await
				.map_err(operation_error);
		}
		let authority = self
			.0
			.home
			.authority
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		authority
			.message_with_files(
				&self.0.store,
				&self.0.run,
				key,
				content,
				files,
				descriptions,
			)
			.await
			.map_err(operation_error)
	}
	async fn attachment(&self, id: &str, model_input: bool) -> Result<Value> {
		use aidash_application::execution::media::{describes_instead, thread_attachment_wav};
		let model = if model_input {
			let model = run_model(&self.0.run)?;
			let observed = observed_media_type(&self.0.run, id);
			if let Some(media_type) = observed
				&& !describes_instead(&model, &media_type)
			{
				thread_attachment_wav(&model, &media_type)?;
			}
			Some(model)
		} else {
			None
		};
		let record = self.0.home.read_tool_record("attachment", id).await?;
		let metadata = record["attachment"].clone();
		if !model_input {
			return Ok(json!({"status":"completed", "metadata":metadata}));
		}
		let media_type = metadata["media_type"]
			.as_str()
			.ok_or(aidash_application::Error::Forbidden)?;
		let model = model.ok_or(aidash_application::Error::Forbidden)?;
		if describes_instead(&model, media_type) {
			// A model that cannot read images gets the words instead: the
			// Image Description, file name and message text, and no selection,
			// so the next inference carries no image (#622 decision 6).
			let message_id = record["message_id"]
				.as_str()
				.ok_or(aidash_application::Error::Forbidden)?;
			let message = self.0.home.read_record("message", message_id).await?;
			let message_text: String = message["content"]
				.as_str()
				.unwrap_or_default()
				.chars()
				.take(aidash_application::semantic::retrieval::ATTACHMENT_MESSAGE_TEXT_CHARS)
				.collect();
			return Ok(json!({"status":"completed", "metadata":metadata,
				"described_instead":{"reason":"this model cannot read images",
					"description":metadata["description"], "message_text":message_text}}));
		}
		let wav = thread_attachment_wav(&model, media_type)?;
		Ok(
			json!({"status":"completed", "metadata":metadata, "selection":{
				"attachment_id":id, "message_id":record["message_id"], "expected_digest":record["sha256"], "wav":wav
			}}),
		)
	}
	async fn attachment_copy(&self, input: Value) -> Result<Value> {
		if !self.0.run.is_local(self.0.store.deployment.as_str()) {
			// Reach: the Home's Thread (ADR 0139, decision 20). The Home keeps the attachment
			// and answers the receiver's pull; the bytes land where the Run's commands run.
			let id: Uuid = serde_json::from_value(input["id"].clone())
				.map_err(|_| aidash_application::Error::Invalid("invalid attachment id".into()))?;
			let record = self
				.0
				.home
				.read_record("attachment", &id.to_string())
				.await?;
			let message_id: Uuid = serde_json::from_value(record["message_id"].clone())
				.map_err(|_| aidash_application::Error::Forbidden)?;
			let digest = record["sha256"]
				.as_str()
				.ok_or(aidash_application::Error::Forbidden)?;
			let media_type = record["attachment"]["media_type"]
				.as_str()
				.ok_or(aidash_application::Error::Forbidden)?
				.to_owned();
			// Fetched before any authority lease is taken: the pull is a Home request. It is
			// bounded at 8 MiB by `/thread-attachment/1/fetch`, which is below the 100 MiB a
			// Thread Attachment may be.
			let (_, _, content) = self
				.0
				.home
				.thread_attachment(id, message_id, digest)
				.await?
				.ok_or(aidash_application::Error::Forbidden)?;
			let authority = self
				.0
				.home
				.tool_authority()
				.ok_or(aidash_application::Error::Forbidden)?;
			return authority
				.copy_pulled_thread_attachment(
					&self.0.store,
					&self.0.run,
					input,
					message_id,
					media_type,
					&content,
				)
				.await
				.map_err(operation_error);
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
			.map_err(|error| match error {
				crate::Error::Forbidden => error.into(),
				other => operation_error(other),
			})
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
	async fn thread_image(
		&self,
		selection: &aidash_domain::media::WorkingImageSelection,
		representation: &str,
	) -> Result<Value> {
		let ctx = &self.0;
		if !ctx.run.is_local(ctx.store.deployment.as_str()) {
			return Err(aidash_application::Error::Forbidden);
		}
		let authority = ctx
			.home
			.authority
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		authority
			.thread_image(&ctx.store, &ctx.run, selection, representation)
			.await
			.map_err(Into::into)
	}
	async fn thread_search(&self, input: Value) -> Result<Value> {
		let ctx = &self.0;
		if !ctx.run.is_local(ctx.store.deployment.as_str()) {
			// The Home searches its own Thread by text.
			return ctx
				.home
				.forward_thread_search(input)
				.await
				.map_err(Into::into);
		}
		let authority = ctx
			.home
			.authority
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		authority
			.thread_search(&ctx.store, &ctx.run, input)
			.await
			.map_err(Into::into)
	}

	async fn read_subnode_record_chunk(
		&self,
		thread: Uuid,
		kind: &str,
		id: &str,
		offset: usize,
		maximum: usize,
	) -> Result<Value> {
		self.0
			.home
			.read_subnode_record_chunk(thread, kind, id, offset, maximum)
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
			// The Home writes only this Agent's participant bank.
			return ctx
				.home
				.forward_memory_mutate(key, changes)
				.await
				.map_err(Into::into);
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
		if !ctx.run.is_local(ctx.store.deployment.as_str()) {
			// The Home's published bank and this Agent's participant bank.
			return ctx
				.home
				.forward_memory_recall(key, query, reflect)
				.await
				.map_err(Into::into);
		}
		if let Some(authority) = &ctx.home.authority {
			let (outcome, left_out) = authority
				.memory_recall(&ctx.store, &ctx.run, key, query.clone(), reflect)
				.await?;
			let mut result = json!(outcome);
			aidash_domain::unconfirmed::attach(&mut result, left_out);
			return Ok(result);
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
		// A connection request of a Run executing for another Home asks the Home's person
		// under the `connection_request` tool, not under `human_request`.
		if kind == aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND
			&& !self.0.run.is_local(self.0.store.deployment.as_str())
		{
			return self
				.0
				.home
				.forward_connection_request(prompt, key)
				.await
				.map_err(Into::into);
		}
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
	async fn intrinsic_subnode_specifications(&self) -> Result<BTreeMap<String, ToolSpec>> {
		let providers = super::bindings::Providers {
			federation: self.0.home.federation.clone(),
			home: self.0.home.clone(),
			scope: self.0.scope.clone(),
		};
		let mut specifications = BTreeMap::new();
		// Journal retrieval, as the Run's tool map receives it (#850 decision 6).
		let handed =
			super::context_journal::handed_to_run(&self.0.store, &self.0.scope, &self.0.run)
				.await?;
		if handed.is_some()
			|| aidash_domain::context::journal_query::offloaded_any(&self.0.run.context)
		{
			for name in aidash_application::registry::system::journal_tools(
				self.0.store.capabilities.0.runner.is_some(),
			) {
				let note = handed.map(|(creator, range)| format!(" The creating Run handed this Task journal range {} through {}. Set run_id to {} and request only this range to inspect it. Omit run_id to inspect your own Journal.", range.from_seq, range.through_seq, creator));
				specifications.insert(
					(*name).to_owned(),
					providers.intrinsic_tool(name, note)?.specification(),
				);
			}
		}
		let Some(authority) = &self.0.home.authority else {
			return Ok(specifications);
		};
		if !authority.is_subnode_member(&self.0.run).await? {
			return Ok(specifications);
		}
		for name in aidash_domain::subnodes::COORDINATION_TOOLS {
			specifications.insert(
				(*name).to_owned(),
				providers
					.coordination_tool(&self.0.run, name)?
					.specification(),
			);
		}
		Ok(specifications)
	}
	async fn direct_skills(&self) -> Result<Vec<DirectSkill>> {
		match self.0.home.tool_authority() {
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
			.tool_authority()
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
		let secret = run_secret(
			&self.0.store,
			&self.0.run,
			&self.0.scope,
			name,
			endpoint,
			owner,
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

/// The named secret of one call a Run makes: the Run's chain-root person's personal secret wins
/// over the shared Tenant Secret, and `SecretOwner::Personal` refuses the shared one. Registry tools
/// and the remote servers of Agent Plugins resolve through this one path.
pub(crate) async fn run_secret(
	store: &crate::store::Store,
	run: &aidash_domain::Run,
	scope: &aidash_application::tenancy::TenancyScope,
	name: &str,
	endpoint: &str,
	owner: aidash_domain::tool::SecretOwner,
	call: aidash_application::tenant_secrets::SecretCall,
) -> Result<aidash_application::tenant_secrets::CallSecret> {
	// The Tenant Secret is resolved in the Run's own Tenant: under Federation
	// it never leaves the owning Tenant's Node. A self-hosted Store reads the
	// value; a Cloud or Dedicated Store mints a broker route instead.
	let unavailable = || {
		if owner == aidash_domain::tool::SecretOwner::Personal {
			aidash_application::tenant_secrets::personal_required(name)
		} else {
			aidash_application::Error::Invalid(aidash_domain::tenant_secrets::UNAVAILABLE.into())
		}
	};
	let (Some(service), Some(tenant)) = (&store.tenant_secrets, scope.tenant_name()) else {
		return Err(unavailable());
	};
	let person =
		crate::apps::identity::repositories::oauth_connections::owner_of_run(store, run).await?;
	if person
		.as_ref()
		.is_some_and(|person| person.tenant != tenant.as_str())
	{
		return Err(aidash_application::Error::Forbidden);
	}
	service
		.call_secret_for(
			tenant.as_str(),
			person.as_ref().map(|person| person.subject.as_str()),
			name,
			owner,
			endpoint,
			call,
		)
		.await
}
