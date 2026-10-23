//! Native source adapters preserve their owning authorization and retrieval paths.
use super::*;
use aidash_application::{context::source::fetch, ports::context_source::ContextSourceProvider};
use aidash_domain::{
	context::source::*,
	decision::SourcePin,
	registry::{
		bindings::{QualifiedRef, ResolvedBinding},
		rules::digest,
	},
};
use serde_json::json;

/// Each item retains its owning source identity and the pointer used only to
/// reconstruct the current request. Array positions never form its identity.
#[derive(Clone)]
struct Fragment {
	candidate: SourceCandidate,
	pointer: String,
}
struct NativeContribution<'a> {
	environment: &'a Environment<'a>,
	run: &'a Run,
	authorized_content: Value,
	slot: Slot,
	fragments: Vec<Fragment>,
}
#[async_trait]
impl ContextSourceProvider for NativeContribution<'_> {
	async fn fetch(&self, request: &SourceRequest) -> Result<SourceBatch> {
		let candidates = self
			.fragments
			.iter()
			.filter(|fragment| fragment.candidate.identity.source == request.source)
			.map(|fragment| fragment.candidate.clone())
			.collect::<Vec<_>>();
		let outcome = if candidates.is_empty() {
			SourceOutcome::Empty
		} else {
			SourceOutcome::Fetched
		};
		let batch = SourceBatch {
			candidates,
			outcome,
		};
		if serde_json::to_vec(&batch)?.len() > request.max_bytes {
			if request.declaration.required_home {
				return Err(aidash_application::Error::Context(
					aidash_domain::context::recovery::Failure::ContextUnreducible,
				));
			}
			return Ok(SourceBatch {
				candidates: vec![],
				outcome: SourceOutcome::Deferred,
			});
		}
		Ok(batch)
	}
	async fn detail(&self, request: &DetailRequest) -> Result<SourceBatch> {
		let candidate = self
			.fragments
			.iter()
			.map(|fragment| &fragment.candidate)
			.find(|candidate| candidate.identity == request.identity)
			.ok_or_else(|| {
				aidash_application::Error::Conflict("source detail revision changed".into())
			})?;
		if candidate.detail.as_ref() != Some(&request.route) {
			return Err(aidash_application::Error::Forbidden);
		}
		let snapshot = self.run.context.binding_snapshot.as_ref().ok_or_else(|| {
			aidash_application::Error::Invalid("source detail lacks snapshot".into())
		})?;
		let binding = snapshot
			.bindings
			.iter()
			.find(|binding| {
				binding.excluded_reason.is_none() && binding.identity == request.route.tool
			})
			.ok_or(aidash_application::Error::Forbidden)?;
		let alias = binding
			.alias
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		let tools = self.environment.binding_resolver().tools(self.run).await?;
		let tool = tools
			.get(alias)
			.ok_or(aidash_application::Error::Forbidden)?;
		let contract = tool.contract();
		if contract.behavior.effect != aidash_domain::tool::ToolEffect::ReadOnly {
			return Err(aidash_application::Error::Forbidden);
		}
		self.environment.recheck_source_observation(self.run,&json!({"semantic_memory":if matches!(self.slot,Slot::Semantic){self.authorized_content.clone()}else{Value::Null}})).await?;
		let mut arguments = request.route.arguments.clone();
		if let Some(max_chars) = arguments.get("max_chars").and_then(Value::as_u64) {
			arguments["max_chars"] = json!(max_chars.min(request.request.max_bytes as u64));
		}
		let call = ToolCall {
			id: format!("source-detail-{}", request.identity.key()),
			name: alias.clone(),
			arguments: arguments.clone(),
		};
		if let Some(authority) = self.environment.authority() {
			authority.tool(&call, &contract).await?;
		}
		let mut output = tool
			.invoke(self.run, arguments, &request.identity.key())
			.await?;
		let original = output["content"]
			.as_str()
			.or_else(|| output["text"].as_str())
			.unwrap_or_default()
			.to_owned();
		let field = if output.get("content").is_some() {
			"content"
		} else {
			"text"
		};
		let mut size = original.len();
		loop {
			let text = utf8_prefix(&original, size);
			if !original.is_empty() {
				output[field] = json!(text);
			}
			let offset = output["offset"].as_u64().unwrap_or(0) as usize;
			let partial =
				text.len() < original.len() || !output["next_offset"].is_null() || offset > 0;
			let mut detailed = candidate.clone();
			detailed.content = output.clone();
			detailed.preview = utf8_prefix(text, MAX_PREVIEW_BYTES).into();
			if !text.is_empty() {
				detailed.identity.range = Some([offset, offset + text.len()]);
			}
			detailed.completeness = if partial {
				Completeness::Partial
			} else {
				Completeness::Complete
			};
			let batch = SourceBatch {
				candidates: vec![detailed],
				outcome: if partial {
					SourceOutcome::Truncated
				} else {
					SourceOutcome::Fetched
				},
			};
			if serde_json::to_vec(&batch)?.len() <= request.request.max_bytes {
				return Ok(batch);
			}
			if size == 0 {
				return Ok(SourceBatch {
					candidates: vec![],
					outcome: SourceOutcome::Deferred,
				});
			}
			size /= 2;
		}
	}
}

#[derive(Clone, Copy)]
enum Slot {
	ResidentSkills,
	RegistrySkills,
	References,
	Skills,
	Semantic,
	Observation,
}
impl Slot {
	fn name(self) -> &'static str {
		match self {
			Self::References => "references",
			Self::Skills => "skills",
			Self::RegistrySkills => "registry_skills",
			Self::ResidentSkills => "resident_skills",
			Self::Semantic => "semantic_memory",
			Self::Observation => "workspace",
		}
	}
	fn matches(self, binding: &ResolvedBinding) -> bool {
		use aidash_domain::registry::bindings::sources::{NativeContext, NativeSource};
		if matches!(self, Self::ResidentSkills)
			&& binding.excluded_reason.is_none()
			&& binding.definition.kind == "skill"
		{
			return true;
		}
		if matches!(self, Self::RegistrySkills) {
			return binding.excluded_reason.is_none() && binding.definition.kind == "skill";
		}
		if binding.excluded_reason.is_some() {
			return false;
		}
		if binding.definition.config.get("schema_version").is_some()
			&& matches!(binding.definition.kind.as_str(), "memory" | "source")
		{
			let Ok(context) =
				serde_json::from_value::<NativeContext>(binding.definition.config.clone())
			else {
				return false;
			};
			return matches!(
				(self, context.source),
				(
					Self::References,
					NativeSource::PrivateReferences { .. }
						| NativeSource::ReferenceAttachments { .. }
				) | (
					Self::Skills | Self::ResidentSkills,
					NativeSource::SkillAttachments { .. } | NativeSource::SkillRoots { .. }
				) | (
					Self::Semantic,
					NativeSource::ConversationMemory { .. }
						| NativeSource::SemanticMemory { .. }
						| NativeSource::WorkspaceRetrieval { .. }
				) | (Self::Observation, NativeSource::WorkspaceObservation { .. })
			);
		}
		matches!(self, Self::Semantic)
			&& matches!(binding.definition.kind.as_str(), "memory" | "source")
	}
}

impl Environment<'_> {
	pub(super) async fn collect_context_source(
		&self,
		read: ContextSourceRead<'_>,
		worker: Uuid,
	) -> SemanticContextRead {
		let resident_blocks = if let ContextSourceRead::ResidentSkills { blocks, .. } = &read {
			Some(*blocks)
		} else {
			None
		};
		let (run, slot, max_bytes) = match &read {
			ContextSourceRead::ResidentSkills { run, .. } => {
				(*run, Slot::ResidentSkills, MAX_SOURCE_BYTES)
			}
			ContextSourceRead::RegistrySkills { run } => {
				(*run, Slot::RegistrySkills, MAX_SOURCE_BYTES)
			}
			ContextSourceRead::References { run, .. } => (*run, Slot::References, MAX_SOURCE_BYTES),
			ContextSourceRead::Skills { run } => (*run, Slot::Skills, MAX_SOURCE_BYTES),
			ContextSourceRead::Semantic { run, budget, .. } => (*run, Slot::Semantic, *budget),
			ContextSourceRead::WorkspaceObservation { run, .. } => {
				(*run, Slot::Observation, MAX_SOURCE_BYTES)
			}
		};
		let prepare = || -> Result<Vec<(QualifiedRef, SourceDeclaration, SourcePin)>> {
			let snapshot = run.context.binding_snapshot.as_ref().ok_or_else(|| {
				aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
			})?;
			let mut declarations = vec![];
			for binding in snapshot
				.bindings
				.iter()
				.filter(|binding| slot.matches(binding))
			{
				let declaration = match serde_json::from_value::<
					aidash_domain::registry::bindings::sources::NativeContext,
				>(binding.definition.config.clone())
				{
					Ok(native) => native.declaration(&binding.identity),
					Err(_) => serde_json::from_value::<aidash_domain::memory::SourceConfig>(
						binding.definition.config.clone(),
					)
					.ok()
					.and_then(|source| source.context_source)
					.unwrap_or(SourceDeclaration {
						provider: "aidash.native-context/1".into(),
						operation: if matches!(slot, Slot::RegistrySkills | Slot::ResidentSkills) {
							"skills"
						} else {
							slot.name()
						}
						.into(),
						tools: SourceTools::default(),
						pin: PinPolicy::Always,
						volatility: Volatility::Step,
						query: QueryPolicy::default(),
						refresh: RefreshPolicy::default(),
						disclosure: SourceDisclosure::default(),
						required_home: false,
					}),
				};
				declarations.push((
					binding.identity.clone(),
					declaration,
					SourcePin {
						resource: format!(
							"registry:{}/{}/{}",
							binding.identity.registry_deployment,
							binding.identity.id,
							binding.identity.version
						),
						revision_digest: binding.digest.clone(),
					},
				));
			}
			// Required-Home is an admission property, never a user opt-in bit.
			if matches!(slot, Slot::Semantic)
				&& (snapshot.remote
					|| self
						.authority
						.as_ref()
						.is_some_and(|authority| authority.is_remote()))
			{
				for (_, declaration, _) in &mut declarations {
					declaration.required_home = true;
					if matches!(
						declaration.pin,
						PinPolicy::Scored {
							mode: aidash_domain::decision::Mode::Enforce,
							..
						} | PinPolicy::OnDemand
					) {
						return Err(aidash_application::Error::Invalid(
							"required-Home context must retain its mandatory delivery".into(),
						));
					}
				}
			}
			Ok(declarations)
		};
		let declarations = match prepare() {
			Ok(declarations) => declarations,
			Err(error) => {
				return SemanticContextRead {
					selections: vec![],
					contributions: vec![],
					workspace_retrieved: false,
					result: Err(error),
				};
			}
		};
		if !declarations.is_empty()
			&& declarations
				.iter()
				.all(|(_, declaration, _)| matches!(declaration.pin, PinPolicy::OnDemand))
		{
			let empty = match slot {
				Slot::References => Some(json!([])),
				Slot::Skills | Slot::RegistrySkills | Slot::ResidentSkills => Some(json!("")),
				_ => None,
			};
			return SemanticContextRead {
				selections: vec![],
				contributions: vec![],
				workspace_retrieved: false,
				result: Ok(empty),
			};
		}
		let mut result = match read {
			ContextSourceRead::ResidentSkills { blocks, .. } => SemanticContextRead {
				selections: vec![],
				contributions: vec![],
				workspace_retrieved: false,
				result: Ok(Some(json!(
					blocks
						.iter()
						.map(|block| block.text.as_str())
						.collect::<String>()
				))),
			},
			ContextSourceRead::RegistrySkills { run } => {
				let read: Result<_> = async {
					let mut text = String::new();
					for binding in &run
						.context
						.binding_snapshot
						.as_ref()
						.ok_or_else(|| {
							aidash_application::Error::Invalid("Skill source lacks snapshot".into())
						})?
						.bindings
					{
						if binding.excluded_reason.is_none() && binding.definition.kind == "skill" {
							let entry = self
								.catalog()
								.get_for_run(run, &binding.identity.id, &binding.identity.version)
								.await?;
							text.push_str(&format!(
								"\nSkill {}@{}:\n{}",
								binding.identity.id,
								binding.identity.version,
								self.catalog().skill_instructions(&entry)?
							));
						}
					}
					Ok(Some(json!(text)))
				}
				.await;
				SemanticContextRead {
					selections: vec![],
					contributions: vec![],
					workspace_retrieved: false,
					result: read,
				}
			}

			ContextSourceRead::References { entry, .. } => SemanticContextRead {
				selections: vec![],
				contributions: vec![],
				workspace_retrieved: false,
				result: self.documents(entry).await.map(Some),
			},
			ContextSourceRead::Skills { run } => SemanticContextRead {
				selections: vec![],
				contributions: vec![],
				workspace_retrieved: false,
				result: self.skill_context(run).await.map(|text| Some(json!(text))),
			},
			ContextSourceRead::WorkspaceObservation { run, offset, limit } => SemanticContextRead {
				selections: vec![],
				contributions: vec![],
				workspace_retrieved: false,
				result: self.home(run).observation(offset, limit).await.map(Some),
			},
			ContextSourceRead::Semantic {
				run,
				task,
				inputs,
				budget,
				entry,
				key,
				degrade_on_outage,
			} => {
				self.semantic_context(run, task, inputs, budget, entry, key, degrade_on_outage)
					.await
			}
		};
		let Ok(Some(content)) = &result.result else {
			return result;
		};
		let content = content.clone();
		let assembled: Result<_> = async {
			let provider = self
				.partition_context_sources(run, slot, &content, &declarations, resident_blocks)
				.await?;
			let mut selected = std::collections::BTreeSet::new();
			let mut retained = false;
			for (source, declaration, _) in declarations {
				let (query, query_failure) = if matches!(declaration.pin, PinPolicy::Scored { .. })
				{
					match self.source_query(run, declaration.query).await {
						Ok(query) => (query, None),
						Err(
							aidash_application::Error::Forbidden
							| aidash_application::Error::Unauthorized,
						) => (
							query_material(declaration.query, "", &[], MAX_SOURCE_BYTES),
							Some(SelectionFailure::ForbiddenDisclosure),
						),
						Err(_) => (
							query_material(declaration.query, "", &[], MAX_SOURCE_BYTES),
							Some(SelectionFailure::AuthorityUnavailable),
						),
					}
				} else {
					(
						query_material(declaration.query, "", &[], MAX_SOURCE_BYTES),
						None,
					)
				};
				let request = SourceRequest {
					run_id: run.id,
					source,
					declaration,
					boundary: format!("{}:{}:{}", run.step, run.observed_input_seq, run.revision),
					query,
					max_bytes: MAX_SOURCE_BYTES,
				};
				let source_fragments = provider
					.fragments
					.iter()
					.filter(|fragment| fragment.candidate.identity.source == request.source)
					.cloned()
					.collect::<Vec<_>>();
				for fragments in source_fragments.chunks(MAX_CANDIDATES) {
					let batch_provider = NativeContribution {
						environment: self,
						run,
						authorized_content: content.clone(),
						slot,
						fragments: fragments.to_vec(),
					};
					let mut batch = fetch(&batch_provider, &request).await?;
					if let PinPolicy::Scored { mode, decider } = &request.declaration.pin {
						let selection = self
							.select_context_source(
								run,
								worker,
								decider,
								*mode,
								&request,
								&batch.candidates,
								&content,
								query_failure,
								max_bytes,
							)
							.await?;
						let failure = selection.evidence.failure;
						result.selections.push(selection.evidence);
						if *mode == aidash_domain::decision::Mode::Enforce
							&& let Some(failure) = failure
						{
							return Err(aidash_application::Error::Context(failure.pause_reason()));
						}
						batch.candidates.retain(|candidate| {
							selection.selected.contains(&candidate.identity.key())
						});
					}
					retained |= !batch.candidates.is_empty();
					selected.extend(
						batch
							.candidates
							.iter()
							.map(|candidate| candidate.identity.key()),
					);
					result
						.contributions
						.extend(batch.candidates.into_iter().map(SourceContribution::from));
				}
			}
			let mut filtered = content.clone();
			let mut removals = provider
				.fragments
				.iter()
				.filter(|fragment| {
					!provider.fragments.iter().any(|other| {
						other.pointer == fragment.pointer
							&& selected.contains(&other.candidate.identity.key())
					})
				})
				.map(|fragment| fragment.pointer.clone())
				.collect::<Vec<_>>();
			removals.sort_by(|left, right| pointer_order(right).cmp(&pointer_order(left)));
			removals.dedup();
			for contribution in &mut result.contributions {
				if let Some(fragment) = provider
					.fragments
					.iter()
					.find(|fragment| fragment.candidate.identity == contribution.identity)
				{
					contribution.placement = Some(SourcePlacement {
						slot: slot.name().into(),
						pointer: remap_pointer(&fragment.pointer, &removals),
					});
				}
			}
			for pointer in removals {
				remove_pointer(&mut filtered, &pointer)?;
			}
			Ok((retained, filtered))
		}
		.await;
		match assembled {
			Ok((_, filtered)) => result.result = Ok(Some(filtered)),
			Err(error) => result.result = Err(error),
		}
		result
	}

	async fn source_query(&self, run: &Run, policy: QueryPolicy) -> Result<QueryMaterial> {
		let home = self.home(run);
		let task = home.task().await?;
		let inputs = self.run_store.run_inputs(run.id).await?;
		let mut accepted = vec![];
		for input in inputs {
			let Some(id) = input.message_id else {
				continue;
			};
			let message: Message =
				serde_json::from_value(home.read_record("message", &id.to_string()).await?)?;
			if message.workspace_id != run.workspace_id || message.content != input.content {
				return Err(aidash_application::Error::Conflict(
					"run input message binding changed".into(),
				));
			}
			accepted.push(AcceptedQueryInput {
				sequence: input.seq,
				text: message.content,
			});
		}
		Ok(query_material(
			policy,
			&format!("{}\n{}", task.title, task.description),
			&accepted,
			MAX_SOURCE_BYTES,
		))
	}
}

struct SourceAuthority<'a> {
	environment: &'a Environment<'a>,
	run: &'a Run,
	allowed: Vec<SourcePin>,
	content: Value,
	config: aidash_domain::decision::DeciderConfig,
}
#[async_trait]
impl aidash_application::ports::decision::DecisionAuthority for SourceAuthority<'_> {
	async fn check(
		&self,
		_: &aidash_domain::decision::Boundary,
		decider: &aidash_domain::decision::DeciderPin,
		sources: &[SourcePin],
	) -> Result<aidash_application::ports::decision::Approval> {
		if sources != self.allowed {
			return Err(aidash_application::Error::Forbidden);
		}
		let authority = self
			.environment
			.authority
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		super::super::decisions::require_approved(authority.guard, decider).await?;
		authority
			.guard
			.local_authority()
			.ok_or(aidash_application::Error::Forbidden)?
			.context_source_disclosure(decider, &self.config, sources)
			.await?;
		self.environment
			.recheck_source_observation(self.run, &self.content)
			.await?;
		Ok(aidash_application::ports::decision::Approval {
			restrictions: Default::default(),
			state_retention: self.config.state_retention.clone(),
		})
	}
	async fn read(&self, _: &aidash_domain::decision::Evidence, _: bool) -> Result<()> {
		Err(aidash_application::Error::Forbidden)
	}
}
impl Environment<'_> {
	async fn select_context_source(
		&self,
		run: &Run,
		worker: Uuid,
		reference: &QualifiedRef,
		mode: aidash_domain::decision::Mode,
		request: &SourceRequest,
		candidates: &[SourceCandidate],
		authorized_content: &Value,
		query_failure: Option<SelectionFailure>,
		budget: usize,
	) -> Result<SourceSelection> {
		use super::super::decisions::{ChainAllowance, RunDecisionJournal};
		use aidash_application::decision::{
			DecisionGate, Evaluation, source_selection_input_digest, source_selection_pins,
		};
		use aidash_domain::decision::{Boundary, Hook};
		let (bound, definition) = run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| {
				aidash_application::Error::Invalid(
					"source selection lacks admitted snapshot".into(),
				)
			})?
			.bound_decider(Hook::ContextSelection)?
			.ok_or_else(|| {
				aidash_application::Error::Context(SelectionFailure::Unapproved.pause_reason())
			})?;
		if &bound.pin.identity != reference || bound.config.mode != mode {
			return Err(aidash_application::Error::Invalid(
				"source selection differs from admitted Decider reference or mode".into(),
			));
		}
		let boundary = Boundary {
			node: self.node_id().into(),
			run: run.id,
			step: run.step,
			run_revision: run.revision,
			input_revision: run.observed_input_seq,
			worker,
			input_digest: source_selection_input_digest(candidates, &request.query, budget),
			cycle: run.recovery.decision_cycle,
			retry: run.recovery.decider_retry,
		};
		let unavailable = || SourceSelection {
			selected: if mode == aidash_domain::decision::Mode::Shadow {
				candidates.iter().map(|c| c.identity.key()).collect()
			} else {
				vec![]
			},
			evidence: SelectionEvidence {
				configuration: bound.config.clone(),
				binding_restrictions: bound.restrictions.clone(),
				restrictions: bound.restrictions.clone(),
				attempt_routes: Default::default(),
				source_pins: source_selection_pins(candidates, &request.query),
				window: budget,
				candidates: candidates
					.iter()
					.map(|candidate| SelectionCandidate {
						identity: candidate.identity.clone(),
						mandatory: candidate.mandatory,
					})
					.collect(),
				boundary: boundary.clone(),
				decider: bound.pin.clone(),
				mode,
				query_digest: request.query.query_digest.clone(),
				candidate_digest: digest(&json!(candidates)),
				state_digest: digest(&Value::Null),
				route: None,
				threshold: None,
				questions: Default::default(),
				answers: Default::default(),
				attempts: vec![],
				proposed: vec![],
				selected: if mode == aidash_domain::decision::Mode::Shadow {
					candidates.iter().map(|c| c.identity.key()).collect()
				} else {
					vec![]
				},
				failure: Some(query_failure.unwrap_or(SelectionFailure::AuthorityUnavailable)),
			},
		};
		if query_failure.is_some() {
			return Ok(unavailable());
		}
		let Some(authority) = &self.authority else {
			return Ok(unavailable());
		};
		// Required-Home execution may observe under its existing local contract;
		// it cannot invent a new remote decision or ancestor allowance.
		if authority.guard.local_authority().is_none() {
			return Ok(unavailable());
		}
		let journal = RunDecisionJournal::new(
			&self.federation.store,
			run.clone(),
			bound.allowance(),
			ChainAllowance(bound.allowance()),
		)?;
		let ports = self
			.decision_ports(
				authority,
				run,
				bound.clone(),
				definition.clone(),
				Box::new(journal),
			)
			.await?;
		let pins = source_selection_pins(candidates, &request.query);
		let source_authority = SourceAuthority {
			environment: self,
			run,
			allowed: pins.clone(),
			content: json!({"semantic_memory": if request.declaration.operation.contains("memory") || request.declaration.operation=="workspace_retrieval" {authorized_content.clone()} else {Value::Null}}),
			config: bound.config.clone(),
		};
		let disclosure = aidash_application::ports::decision::Disclosure {
			sources: pins,
			..Default::default()
		};
		DecisionGate {
			authority: &source_authority,
			journal: ports.journal.as_ref(),
			provider: ports.provider.as_ref(),
		}
		.select_sources(
			candidates,
			&request.query,
			&Evaluation {
				boundary: &boundary,
				decider: &bound,
				definition: &definition,
				disclosure: &disclosure,
				now: ports.journal.now(),
			},
			budget,
		)
		.await
	}
}

fn pointer_order(pointer: &str) -> Vec<(String, usize)> {
	pointer
		.split('/')
		.skip(1)
		.map(|part| {
			(
				if part.parse::<usize>().is_ok() {
					String::new()
				} else {
					part.into()
				},
				part.parse().unwrap_or(0),
			)
		})
		.collect()
}
fn remove_pointer(value: &mut Value, pointer: &str) -> Result<()> {
	if let Some(range) = pointer.strip_prefix("/@text/") {
		let (start, end) = range.split_once('/').ok_or_else(|| {
			aidash_application::Error::Invalid("Skill source range is invalid".into())
		})?;
		let start = start.parse::<usize>().map_err(|_| {
			aidash_application::Error::Invalid("Skill source range is invalid".into())
		})?;
		let end = end.parse::<usize>().map_err(|_| {
			aidash_application::Error::Invalid("Skill source range is invalid".into())
		})?;
		let text = value
			.as_str()
			.ok_or_else(|| aidash_application::Error::Invalid("Skill source is not text".into()))?;
		if text.get(start..end).is_none() {
			return Err(aidash_application::Error::Conflict(
				"Skill source range changed".into(),
			));
		}
		*value = json!(format!("{}{}", &text[..start], &text[end..]));
		return Ok(());
	}
	let (parent, last) = pointer
		.rsplit_once('/')
		.ok_or_else(|| aidash_application::Error::Invalid("source pointer is not rooted".into()))?;
	let parent = value.pointer_mut(parent).ok_or_else(|| {
		aidash_application::Error::Conflict("source candidate placement changed".into())
	})?;
	match parent {
		Value::Array(items) => {
			let index = last.parse::<usize>().map_err(|_| {
				aidash_application::Error::Invalid("source array placement is invalid".into())
			})?;
			if index >= items.len() {
				return Err(aidash_application::Error::Conflict(
					"source candidate placement changed".into(),
				));
			}
			items.remove(index);
		}
		Value::Object(fields) => {
			fields.remove(last);
		}
		_ => {
			return Err(aidash_application::Error::Invalid(
				"source placement is not a container".into(),
			));
		}
	}
	Ok(())
}
impl Environment<'_> {
	async fn partition_context_sources<'a>(
		&'a self,
		run: &'a Run,
		slot: Slot,
		content: &Value,
		declarations: &[(QualifiedRef, SourceDeclaration, SourcePin)],
		resident_blocks: Option<&[PreparedSkillSource]>,
	) -> Result<NativeContribution<'a>> {
		use aidash_domain::registry::bindings::sources::{NativeContext, NativeSource};
		let snapshot = run.context.binding_snapshot.as_ref().ok_or_else(|| {
			aidash_application::Error::Invalid("source partition lacks snapshot".into())
		})?;
		let mut fragments = vec![];
		let mut reference_offset = 0;
		for (source, declaration, pin) in declarations {
			let binding = snapshot
				.bindings
				.iter()
				.find(|binding| &binding.identity == source)
				.ok_or_else(|| {
					aidash_application::Error::Invalid("source declaration not admitted".into())
				})?;
			let native =
				serde_json::from_value::<NativeContext>(binding.definition.config.clone()).ok();
			let mut paths = vec![];
			match slot {
				Slot::ResidentSkills => {
					let mut offset = 0;
					for block in resident_blocks.unwrap_or_default() {
						let end = offset + block.text.len();
						let owns = match &block.identity {
							aidash_domain::exposure::CapabilityIdentity::Registry(identity) => {
								identity == source
							}
							aidash_domain::exposure::CapabilityIdentity::DirectSkill {
								skill_id,
								origin,
							} => match native.as_ref().map(|native| &native.source) {
								Some(NativeSource::SkillAttachments { attachments }) => attachments
									.iter()
									.any(|attachment| &attachment.skill_id == skill_id),
								Some(NativeSource::SkillRoots { roots }) => {
									origin.starts_with("area:")
										&& roots
											.iter()
											.any(|root| origin.contains(&format!(":{root}/")))
								}
								_ => false,
							},
							_ => false,
						};
						if owns {
							let resource = SourcePin {
								resource: format!("skill:{}", block.identity),
								revision_digest: block.digest.clone(),
							};
							fragments.push(Fragment {
								pointer: format!("/@text/{offset}/{end}"),
								candidate: SourceCandidate {
									identity: CandidateIdentity {
										source: source.clone(),
										item: format!(
											"resident_skills/{}",
											digest(&json!(block.identity))
										),
										revision: block.digest.clone(),
										range: None,
									},
									content: json!(block.text),
									preview: utf8_prefix(&block.preview, MAX_PREVIEW_BYTES).into(),
									provenance: vec![pin.clone(), resource.clone()],
									dependencies: vec![pin.clone(), resource],
									completeness: Completeness::Complete,
									detail: None,
									mandatory: false,
								},
							});
						}
						offset = end;
					}
				}
				Slot::RegistrySkills => {
					let mut offset = 0;
					for skill in snapshot.bindings.iter().filter(|binding| {
						binding.excluded_reason.is_none() && binding.definition.kind == "skill"
					}) {
						let block = format!(
							"\nSkill {}@{}:\n{}",
							skill.identity.id,
							skill.identity.version,
							self.catalog().skill_instructions(&skill.definition)?
						);
						let end = offset + block.len();
						if &skill.identity == source {
							if content.as_str().and_then(|text| text.get(offset..end))
								!= Some(block.as_str())
							{
								return Err(aidash_application::Error::Conflict(
									"Registry Skill source placement changed".into(),
								));
							}
							fragments.push(Fragment {
								pointer: format!("/@text/{offset}/{end}"),
								candidate: SourceCandidate {
									identity: CandidateIdentity {
										source: source.clone(),
										item: format!("registry_skills/{}", source.id),
										revision: pin.revision_digest.clone(),
										range: None,
									},
									content: json!(block),
									preview: utf8_prefix(
										&self.catalog().skill_instructions(&skill.definition)?,
										MAX_PREVIEW_BYTES,
									)
									.into(),
									provenance: vec![pin.clone()],
									dependencies: vec![pin.clone()],
									completeness: Completeness::Complete,
									detail: None,
									mandatory: false,
								},
							});
						}
						offset = end;
					}
				}
				Slot::References => {
					if let Some(NativeContext {
						source: NativeSource::PrivateReferences { .. },
						..
					}) = &native
					{
						let tenant = self.federation.store.run_tenant(run.id).await?;
						let own = crate::knowledge::load(
							&self.federation.registry,
							&tenant,
							&binding.definition,
						)
						.await?;
						for (index, item) in own
							.as_array()
							.ok_or_else(|| {
								aidash_application::Error::Invalid(
									"private source is not a document list".into(),
								)
							})?
							.iter()
							.enumerate()
						{
							let pointer = format!("/{}", reference_offset + index);
							if content.pointer(&pointer) != Some(item) {
								return Err(aidash_application::Error::Conflict(
									"private source document placement changed".into(),
								));
							}
							paths.push(pointer);
						}
						reference_offset += own.as_array().unwrap().len();
					}
				}
				Slot::Semantic => {
					let workspace = matches!(
						native.as_ref().map(|native| &native.source),
						Some(
							NativeSource::WorkspaceRetrieval { .. }
								| NativeSource::SemanticMemory { .. }
								| NativeSource::ConversationMemory { .. }
						)
					);
					if workspace {
						for prefix in ["/workspace/matches", "/matches", "/result/matches"] {
							if let Some(items) = content.pointer(prefix).and_then(Value::as_array) {
								for (index, item) in items.iter().enumerate() {
									let memory = item["source"]["kind"] == "memory";
									let matches = match native.as_ref().map(|native| &native.source)
									{
										Some(NativeSource::WorkspaceRetrieval { .. }) => !memory,
										Some(
											NativeSource::SemanticMemory { .. }
											| NativeSource::ConversationMemory { .. },
										) => memory,
										_ => false,
									};
									if matches || declaration.required_home {
										paths.push(format!("{prefix}/{index}"));
									}
								}
							}
						}
					}
					// The primary provider and extra Source declarations address only their
					// exact bank/provider. One source's policy cannot select another bank.
					if native.is_none() {
						let config = serde_json::from_value::<aidash_domain::memory::SourceConfig>(
							binding.definition.config.clone(),
						)
						.ok();
						if let Some(banks) =
							content.pointer("/memory/banks").and_then(Value::as_array)
						{
							for (bank_index, bank) in banks.iter().enumerate() {
								let own = if binding.definition.kind == "memory" {
									bank_index == 0
										&& bank["provider"]["id"] == binding.identity.id
										&& bank["provider"]["version"] == binding.identity.version
								} else {
									config.as_ref().is_some_and(|config| {
										bank["provider"]["id"] == config.memory.id
											&& bank["provider"]["version"] == config.memory.version
											&& match config.scope {
												aidash_domain::memory::SourceScope::Workspace => {
													bank["bank"]["participant"].is_null()
												}
												aidash_domain::memory::SourceScope::Participant => {
													!bank["bank"]["participant"].is_null()
												}
											}
									})
								};
								if own
									&& let Some(units) =
										bank.pointer("/recall/units").and_then(Value::as_array)
								{
									for index in 0..units.len() {
										paths.push(format!(
											"/memory/banks/{bank_index}/recall/units/{index}"
										));
									}
								}
							}
						}
					}
					// A remote receipt is an indivisible mandatory contract. Shadow may
					// observe every match; it never rewrites sources or the Home receipt.
					if declaration.required_home && paths.is_empty() {
						paths.push(String::new());
					}
				}
				Slot::Observation => {
					if let Some(fields) = content.as_object() {
						for (field, value) in fields {
							if let Some(items) = value.as_array() {
								for index in 0..items.len() {
									paths.push(format!("/{field}/{index}"));
								}
							}
						}
					}
				}
				Slot::Skills => {
					let Some(authority) = self
						.authority
						.as_ref()
						.and_then(|authority| authority.guard.local_authority())
					else {
						continue;
					};
					let items = authority
						.skill_context_items(&self.federation.store, run)
						.await?;
					let mut offset =
						"\nPinned Skills (select by UUID and origin; use skill_load):\n".len();
					for item in items {
						let end = offset + item.text.len();
						if content.as_str().and_then(|text| text.get(offset..end))
							!= Some(item.text.as_str())
						{
							return Err(aidash_application::Error::Conflict(
								"pinned Skill source placement changed".into(),
							));
						}
						let owns = match native.as_ref().map(|native| &native.source) {
							Some(NativeSource::SkillAttachments { attachments }) => attachments
								.iter()
								.any(|attachment| attachment.skill_id == item.metadata.skill_id),
							Some(NativeSource::SkillRoots { roots }) => {
								item.metadata.origin.starts_with("area:")
									&& roots.iter().any(|root| {
										item.metadata.origin.contains(&format!(":{root}/"))
									})
							}
							_ => false,
						};
						if owns {
							let resource = SourcePin {
								resource: format!("skill:{}", item.metadata.skill_id),
								revision_digest: item.metadata.digest.clone(),
							};
							let detail=declaration.tools.detail.as_ref().map(|tool|DetailRoute{tool:tool.clone(),arguments:json!({"skill_id":item.metadata.skill_id,"digest":item.metadata.digest,"path":"SKILL.md","offset":0,"max_chars":2048}),revision:item.metadata.digest.clone()});
							fragments.push(Fragment {
								pointer: format!("/@text/{offset}/{end}"),
								candidate: SourceCandidate {
									identity: CandidateIdentity {
										source: source.clone(),
										item: format!("skills/{}", item.metadata.skill_id),
										revision: item.metadata.digest.clone(),
										range: None,
									},
									content: json!(item.text),
									preview: utf8_prefix(
										&item.metadata.description,
										MAX_PREVIEW_BYTES,
									)
									.into(),
									provenance: vec![pin.clone(), resource.clone()],
									dependencies: vec![pin.clone(), resource],
									completeness: Completeness::Complete,
									detail,
									mandatory: false,
								},
							});
						}
						offset = end;
					}
				}
			}
			for pointer in paths {
				let item = content
					.pointer(&pointer)
					.ok_or_else(|| {
						aidash_application::Error::Conflict(
							"source item disappeared from authorized result".into(),
						)
					})?
					.clone();
				if item.is_null()
					|| item.as_str() == Some("")
					|| item.as_array().is_some_and(Vec::is_empty)
				{
					continue;
				}
				let revision = item
					.get("revision")
					.map(Value::to_string)
					.unwrap_or_else(|| digest(&item));
				let identity = item
					.get("entry_id")
					.or_else(|| item.get("id"))
					.map(Value::to_string)
					.unwrap_or_else(|| digest(&item));
				let resource = SourcePin {
					resource: format!("{}:{identity}", slot.name()),
					revision_digest: digest(&json!({"revision":revision,"content":item})),
				};
				let preview = item
					.get("text")
					.and_then(Value::as_str)
					.map(str::to_owned)
					.unwrap_or_else(|| item.to_string());
				let detail = declaration.tools.detail.as_ref().and_then(|tool| {
					let kind = item.pointer("/source/kind")?.as_str()?;
					let id = item.pointer("/source/id")?.as_str()?;
					Some(DetailRoute {
						tool: tool.clone(),
						arguments: json!({"kind":kind,"id":id,"offset":0,"max_chars":2048}),
						revision: revision.clone(),
					})
				});
				fragments.push(Fragment {
					pointer,
					candidate: SourceCandidate {
						identity: CandidateIdentity {
							source: source.clone(),
							item: format!("{}/{identity}", slot.name()),
							revision,
							range: None,
						},
						content: item,
						preview: utf8_prefix(&preview, MAX_PREVIEW_BYTES).into(),
						provenance: vec![pin.clone(), resource.clone()],
						dependencies: vec![pin.clone(), resource],
						completeness: Completeness::Complete,
						detail,
						mandatory: declaration.required_home,
					},
				});
			}
		}
		Ok(NativeContribution {
			environment: self,
			run,
			authorized_content: content.clone(),
			slot,
			fragments,
		})
	}
}

fn remap_pointer(pointer: &str, removed: &[String]) -> String {
	if let Some(range) = pointer.strip_prefix("/@text/") {
		let Some((start, end)) = range.split_once('/') else {
			return pointer.into();
		};
		let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>()) else {
			return pointer.into();
		};
		let prior = removed
			.iter()
			.filter_map(|range| {
				range
					.strip_prefix("/@text/")
					.and_then(|range| range.split_once('/'))
					.and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)))
			})
			.filter(|(a, _)| *a < start)
			.map(|(a, b)| b - a)
			.sum::<usize>();
		return format!(
			"/@text/{}/{}",
			start.saturating_sub(prior),
			end.saturating_sub(prior)
		);
	}
	let Some((parent, last)) = pointer.rsplit_once('/') else {
		return pointer.into();
	};
	let Ok(index) = last.parse::<usize>() else {
		return pointer.into();
	};
	let prior = removed
		.iter()
		.filter_map(|removed| removed.rsplit_once('/'))
		.filter(|(other, at)| *other == parent && at.parse::<usize>().is_ok_and(|at| at < index))
		.count();
	format!("{parent}/{}", index.saturating_sub(prior))
}
