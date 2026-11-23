//! Resource attributes come from stored rows and their authoritative thread.
use super::{access::Access, policy::Resource};
use crate::apps::identity::models::DecisionOwner;
use crate::{
	Result,
	domain::{Artifact, Event, Message, RunMetadata, Task},
};
use aidash_application::authorization::{
	visibility::provenance::ReadBlock,
	worker_tasks::{OwnTaskRefusal, own_task_refused},
};
use uuid::Uuid;

impl Access {
	pub(crate) async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		aidash_application::authorization::visibility::resources::task_resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			task,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn task_read(&mut self, id: Uuid) -> Result<Task> {
		let mark = self.call_mark();
		let result = aidash_application::authorization::visibility::resources::task_read(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into);
		// A refusal reached while a Node did not answer is that Node's silence (#751),
		// when this read's own walk met the Node and not an earlier call of the authority.
		result.map_err(|error| self.named_since(mark, error))
	}
	/// Read a named delegation Task. Only a Task created in the Run's own Thread
	/// can disclose why it was refused.
	pub(crate) async fn delegation_task_read(
		&mut self,
		id: Uuid,
		run: &RunMetadata,
	) -> Result<Task> {
		if self.task_source_run(id).await? == Some(run.id)
			&& self.task_thread(id).await? == Some(run.thread_id)
		{
			return self
				.own_task_read(id)
				.await?
				.map_err(|refusal| own_task_refused(id, &refusal).into());
		}
		self.task_read(id).await
	}

	/// [`Self::task_read`] for a Task the calling Run created, saying which check refused
	/// (#1058). A refusal reached while a Node did not answer stays that Node's silence.
	pub(crate) async fn own_task_read(
		&mut self,
		id: Uuid,
	) -> Result<std::result::Result<Task, OwnTaskRefusal>> {
		use aidash_application::authorization::visibility::resources::TaskRefusal;
		let mark = self.call_mark();
		// A block left by an earlier walk of this authority is not this read's.
		self.read_block = None;
		let result = aidash_application::authorization::visibility::resources::task_read_explained(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
		)
		.await
		.map_err(crate::Error::from);
		let block = self.read_block.take();
		match result {
			Ok(Ok(task)) => Ok(Ok(task)),
			Ok(Err(refusal)) => {
				if let silent @ crate::Error::NodeUnreachable { .. } =
					self.named_since(mark, crate::Error::Forbidden)
				{
					return Err(silent);
				}
				Ok(Err(match (refusal, block) {
					(TaskRefusal::Policy, _) => OwnTaskRefusal::Policy,
					(TaskRefusal::Outputs, Some(block)) => {
						if self.reader_recorded(&block).await? {
							OwnTaskRefusal::Read(Some(block))
						} else {
							OwnTaskRefusal::OtherRunRead(Some(block))
						}
					}
					(TaskRefusal::Outputs, None) => OwnTaskRefusal::OtherRunRead(None),
				}))
			}
			Err(error) => Err(self.named_since(mark, error)),
		}
	}
	/// Whether the reading Run recorded `block` itself. A block reached through
	/// another Run's reads is not the reader's to have named by id (#1058).
	async fn reader_recorded(&mut self, block: &ReadBlock) -> Result<bool> {
		let Some(run) = self.read_run else {
			return Ok(false);
		};
		Ok(match block {
			ReadBlock::Source { kind, id } => {
				crate::apps::identity::models::AuthorizationRunRead::for_run(self.tx.as_mut(), run)
					.await?
					.iter()
					.any(|(_, read_kind, read_id)| read_kind == kind && read_id == id)
			}
			ReadBlock::Definition(definition) => {
				crate::apps::identity::models::AuthorizationRunRegistryRead::for_run(
					self.tx.as_mut(),
					run,
				)
				.await?
				.contains(definition)
			}
			ReadBlock::RemoteRead | ReadBlock::SemanticRead => false,
		})
	}
	pub(crate) async fn task_thread(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		use reinhardt::query::{
			Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
		};
		crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("thread_id"))
				.from(Alias::new("tasks"))
				.and_where(Expr::col("id").eq(Expr::value(task)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.tx)
		.await
	}

	/// The Run that created `task`, if one did.
	pub(crate) async fn task_source_run(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		use reinhardt::query::{
			Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
		};
		crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("source_run_id"))
				.from(Alias::new("authorization_task_origins"))
				.and_where(Expr::col("task_id").eq(Expr::value(task)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.tx)
		.await
	}
	pub(crate) async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		aidash_application::authorization::visibility::resources::task_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			task,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn task_summary_visible(
		&mut self,
		thread: &Resource,
		thread_id: Uuid,
		task_id: Uuid,
		created_by: &str,
	) -> Result<bool> {
		aidash_application::authorization::visibility::resources::task_summary_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			thread,
			thread_id,
			task_id,
			created_by,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn related_tasks(
		&mut self,
		thread: Uuid,
		input: &crate::domain::NewTask,
	) -> Result<()> {
		aidash_application::authorization::visibility::resources::related_tasks(
			&mut crate::bootstrap::run_visibility_scope(self),
			thread,
			input,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool> {
		aidash_application::authorization::visibility::resources::artifact_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			artifact,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn output_visible(
		&mut self,
		thread: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<bool> {
		aidash_application::authorization::visibility::provenance::output_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			thread,
			kind,
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn memory_resource(
		&mut self,
		run: impl Into<crate::domain::RunMetadata>,
	) -> Result<Resource> {
		let run = run.into();
		let thread = self.thread(run.thread_id).await?;
		let mut attributes = aidash_application::authorization::visibility::memory_attributes(
			&run,
			thread.attributes,
		);
		let binding = if self.state_binding_exists(run.id) == Some(false) {
			None
		} else {
			crate::apps::knowledge::repositories::bindings::load(&mut **self.tx, &run).await?
		};
		let id = if let Some(binding) = binding {
			attributes["participant_id"] = serde_json::json!(binding.bank.participant);
			attributes["participant_revision"] = serde_json::json!(binding.participant_revision);
			attributes["shared"] = serde_json::json!(false);
			binding
				.bank
				.participant
				.ok_or(crate::Error::Forbidden)?
				.to_string()
		} else {
			run.agent_id.clone()
		};
		Ok(self.resource("memory", id, attributes))
	}

	pub(crate) async fn artifact_creation_resource(
		&mut self,
		task: Uuid,
		creator: &str,
	) -> Result<Resource> {
		aidash_application::authorization::visibility::resources::artifact_creation_resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			task,
			creator,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn message_visible(&mut self, message: &Message) -> Result<bool> {
		aidash_application::authorization::visibility::resources::message_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			message,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn resource_event_visible(&mut self, event: &Event) -> Result<Option<bool>> {
		aidash_application::authorization::visibility::resources::event_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			event,
		)
		.await
		.map_err(Into::into)
	}
}

impl Access {
	pub(crate) async fn track_snapshot(
		&mut self,
		snapshot: &crate::domain::ThreadSnapshot,
	) -> Result<()> {
		aidash_application::authorization::journals::track_snapshot(
			&mut crate::bootstrap::run_visibility_scope(self),
			snapshot,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn track_registry(
		&mut self,
		entries: &[crate::registry::Entry],
	) -> Result<()> {
		aidash_application::authorization::journals::track_registry(
			&mut crate::bootstrap::run_visibility_scope(self),
			entries,
		)
		.await
		.map_err(Into::into)
	}

	/// Fixed built-in memory algorithms are internal to that exact provider;
	/// custom providers and other Registry reads retain their full provenance.
	pub(crate) async fn track_memory_registry(
		&mut self,
		entries: &[crate::registry::Entry],
	) -> Result<()> {
		let built_in = entries
			.iter()
			.any(aidash_domain::registry::bindings::memory_defaults::is_builtin_memory)
			&& !entries.iter().any(|entry| {
				entry.kind == "memory"
					&& !aidash_domain::registry::bindings::memory_defaults::is_builtin_memory(entry)
			});
		let visible: Vec<_> = entries
			.iter()
			.filter(|entry| {
				!built_in
					|| !aidash_domain::registry::bindings::memory_defaults::is_internal_role(entry)
			})
			.cloned()
			.collect();
		self.track_registry(&visible).await
	}

	/// See [`ReadProvenanceScope::open_run_reads`]. An authority that is not
	/// inherited has no Thread boundary to open.
	///
	/// [`ReadProvenanceScope::open_run_reads`]: aidash_application::ports::authorization::visibility::provenance::ReadProvenanceScope::open_run_reads
	pub(crate) async fn open_run_reads(&mut self, run: Uuid) -> Result<bool> {
		if !self.inherited_lease {
			return Ok(false);
		}
		let threads = match self.subnode_threads.get(&run) {
			Some(threads) => threads.clone(),
			None => {
				let threads =
					crate::apps::execution::models::subnodes::SubnodeRecord::threads_of_run(
						&mut **self.tx,
						&self.identity.tenant,
						run,
					)
					.await?;
				self.subnode_threads.insert(run, threads.clone());
				threads
			}
		};
		if threads.is_empty() {
			return Ok(false);
		}
		self.recheck_threads.push(threads);
		Ok(true)
	}
	pub(crate) fn close_run_reads(&mut self) {
		self.recheck_threads.pop();
	}
	pub(crate) fn recheck_reaches(&self, id: Uuid) -> bool {
		self.recheck_threads
			.iter()
			.flatten()
			.any(|opened| *opened == id)
	}
	/// The producers of a record in `thread` whose reads this reader checks. A
	/// Run working in a Subnode's Thread leaves out the Run that created it:
	/// what the creator puts there is its deliberate handoff, and the members
	/// cannot read what the creator had read (ADR 0110). Anyone else checks it.
	pub(crate) async fn producers_to_check(
		&mut self,
		thread: Uuid,
		mut producers: Vec<Uuid>,
	) -> Result<Vec<Uuid>> {
		if producers.is_empty()
			|| self.read_run.is_none()
			|| self.context["thread_id"] != serde_json::json!(thread.to_string())
		{
			return Ok(producers);
		}
		let creator = match self.subnode_creators.get(&thread) {
			Some(creator) => *creator,
			None => {
				let creator =
					crate::apps::execution::models::subnodes::SubnodeRecord::creator_of_thread(
						&mut **self.tx,
						&self.identity.tenant,
						thread,
					)
					.await?;
				self.subnode_creators.insert(thread, creator);
				creator
			}
		};
		if let Some(creator) = creator {
			producers.retain(|producer| *producer != creator);
		}
		Ok(producers)
	}
	/// Walk recorded run dependencies iteratively; cycles between observation
	/// journals must terminate without skipping any resource's current policy.
	pub(crate) async fn run_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let key = (self.node_id.clone(), run, self.authority_context());
		// Runs read each other's outputs, so the same Run is reached by many
		// paths; walking it once per path grows with every read (#404).
		if let Some(verdict) = self.read_verdicts.get(&key) {
			return Ok(verdict);
		}
		let outermost = self.checking_reads.is_idle();
		let Some(_visit) = self.checking_reads.enter(key.clone()) else {
			return Ok(true);
		};
		let walk = self.begin_read_walk(DecisionOwner::Run(run), outermost);
		let coordinator = self.dependency_frontier.is_none();
		if coordinator {
			self.dependency_frontier = Some(vec![]);
		}
		let mut result = self.run_reads_visible_in(run).await;
		if coordinator {
			let pending = self.dependency_frontier.take().unwrap_or_default();
			if matches!(result, Ok(true)) {
				result = self.verify_dependencies(pending).await;
				if matches!(result, Ok(false)) {
					self.read_block.get_or_insert(ReadBlock::RemoteRead);
				}
			}
		}
		// A `false` reached while a Node did not answer may be that Node's
		// silence; it is kept like an abandoned check, not as a verdict (#751).
		let unconfirmed = matches!(result, Ok(false)) && !self.unavailable_peers.is_empty();
		match result {
			Ok(_) if unconfirmed && outermost => self.read_verdicts.abandon(),
			Ok(_) if unconfirmed => {}
			Ok(verdict) => self.read_verdicts.record(key, verdict, outermost),
			Err(_) if outermost => self.read_verdicts.abandon(),
			Err(_) => {}
		}
		self.end_read_walk(walk, result).await
	}
	async fn run_reads_visible_in(&mut self, run: Uuid) -> Result<bool> {
		let block = aidash_application::authorization::visibility::provenance::run_reads_block(
			&mut crate::bootstrap::run_visibility_scope(self),
			run,
		)
		.await?;
		// A nested walk records first, so this keeps the read that actually stopped it.
		match block {
			Some(block) => {
				self.read_block.get_or_insert(block);
				Ok(false)
			}
			None => Ok(true),
		}
	}
	pub(crate) async fn grant_reads_visible(&mut self, grant: Uuid) -> Result<bool> {
		aidash_application::authorization::journals::grant_reads_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			grant,
		)
		.await
		.map_err(Into::into)
	}
}
