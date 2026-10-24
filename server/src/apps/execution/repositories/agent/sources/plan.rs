//! Compile admitted declarations into provider-owned lifecycle handles. The
//! grouping preserves the established native renderer and its request bytes;
//! adding a native adapter does not add an operator branch.
use super::*;

impl Slot {
	pub(super) fn automatic_matches(self, binding: &ResolvedBinding) -> bool {
		self.matches(binding)
			&& !(matches!(binding.definition.kind.as_str(), "memory" | "source")
				&& binding
					.definition
					.config
					.get("context_source")
					.and_then(|config| {
						serde_json::from_value::<SourceDeclaration>(config.clone()).ok()
					})
					.is_some_and(|declaration| matches!(declaration.pin, PinPolicy::OnDemand)))
	}
	fn phase(self) -> ContextSourcePhase {
		match self {
			Self::ResidentSkills | Self::RegistrySkills | Self::Skills => {
				ContextSourcePhase::Instructions
			}
			Self::References => ContextSourcePhase::FixedData,
			Self::Observation => ContextSourcePhase::SnapshotData,
			Self::Semantic => ContextSourcePhase::BudgetedData,
		}
	}
	fn cache(self) -> ContextSourceCache {
		match self {
			Self::ResidentSkills => ContextSourceCache::Boundary,
			Self::Skills => ContextSourceCache::Revision,
			Self::Semantic => ContextSourceCache::Retrieval,
			_ => ContextSourceCache::None,
		}
	}
	fn target(self) -> ContextSourceTarget {
		match self {
			Self::ResidentSkills | Self::RegistrySkills | Self::Skills => {
				ContextSourceTarget::Instructions
			}
			Self::References => ContextSourceTarget::Pinned("reference_documents".into()),
			_ => ContextSourceTarget::Pinned(self.name().into()),
		}
	}
}

impl Environment<'_> {
	pub(in super::super) fn native_context_source_plan(
		&self,
		run: &Run,
	) -> Result<Vec<PlannedContextSource>> {
		let snapshot = run.context.binding_snapshot.as_deref().ok_or_else(|| {
			aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
		})?;
		let agent = snapshot
			.definitions
			.iter()
			.find(|definition| definition.identity == snapshot.agent)
			.ok_or_else(|| aidash_application::Error::Invalid("snapshot lacks its Agent".into()))?;
		let bindings: aidash_domain::registry::bindings::AgentBindings =
			serde_json::from_value(agent.definition.config.clone())?;
		let deferred = bindings.exposure_policy().budgets().is_some();
		let local_skills = self.home(run).has_local_authority()
			&& snapshot.bindings.iter().any(|binding| {
				binding.excluded_reason.is_none() && binding.alias.as_deref() == Some("skill_list")
			});
		let snapshot_digest = digest(&serde_json::to_value(snapshot)?);
		if snapshot.remote
			|| self
				.authority
				.as_ref()
				.is_some_and(|authority| authority.is_remote())
		{
			for binding in snapshot
				.bindings
				.iter()
				.filter(|binding| Slot::Semantic.matches(binding))
			{
				if binding
					.definition
					.config
					.get("context_source")
					.and_then(|config| {
						serde_json::from_value::<SourceDeclaration>(config.clone()).ok()
					})
					.is_some_and(|declaration| {
						matches!(
							declaration.pin,
							PinPolicy::OnDemand
								| PinPolicy::Scored {
									mode: aidash_domain::decision::Mode::Enforce,
									..
								}
						)
					}) {
					return Err(aidash_application::Error::Invalid(
						"required-Home context must retain its mandatory delivery".into(),
					));
				}
			}
		}
		let mut plan = vec![];
		// Host compatibility order is independent of the order in which an Agent
		// declared groups. Within each group the recorded binding order survives.
		for slot in [
			Slot::ResidentSkills,
			Slot::RegistrySkills,
			Slot::Skills,
			Slot::References,
			Slot::Observation,
			Slot::Semantic,
		] {
			if (matches!(slot, Slot::ResidentSkills) && !deferred)
				|| (matches!(slot, Slot::RegistrySkills | Slot::Skills) && deferred)
				|| (matches!(slot, Slot::Skills) && !local_skills)
			{
				continue;
			}
			let admitted = snapshot
				.bindings
				.iter()
				.filter(|binding| slot.automatic_matches(binding))
				.collect::<Vec<_>>();
			if admitted.is_empty() {
				continue;
			}
			let refresh_each_step = admitted.iter().any(|binding| {
				binding
					.definition
					.config
					.get("context_source")
					.and_then(|config| {
						serde_json::from_value::<SourceDeclaration>(config.clone()).ok()
					})
					.is_some_and(|declaration| {
						declaration.volatility == Volatility::Step
							&& !matches!(declaration.pin, PinPolicy::OnDemand)
					})
			});
			plan.push(PlannedContextSource {
				handle: ContextSourceHandle::new(format!(
					"{}:{snapshot_digest}:{}",
					run.id,
					slot.name()
				)),
				phase: slot.phase(),
				target: slot.target(),
				cache: slot.cache(),
				contribution_slot: slot.name().into(),
				sources: admitted
					.iter()
					.map(|binding| binding.identity.clone())
					.collect(),
				refresh_each_step,
			});
		}
		Ok(plan)
	}

	pub(in super::super) fn planned_context_slot(
		&self,
		run: &Run,
		handle: &ContextSourceHandle,
	) -> Result<Slot> {
		let planned = self.native_context_source_plan(run)?;
		let plan = planned
			.iter()
			.find(|plan| plan.handle == *handle)
			.ok_or(aidash_application::Error::Forbidden)?;
		// Only this native provider owns these opaque handles. This dispatch never
		// crosses the application port into the Harness.
		match plan.contribution_slot.as_str() {
			"resident_skills" => Ok(Slot::ResidentSkills),
			"registry_skills" => Ok(Slot::RegistrySkills),
			"skills" => Ok(Slot::Skills),
			"references" => Ok(Slot::References),
			"workspace" => Ok(Slot::Observation),
			"semantic_memory" => Ok(Slot::Semantic),
			_ => Err(aidash_application::Error::Forbidden),
		}
	}

	pub(super) async fn native_context_payload(
		&self,
		run: &Run,
		slot: Slot,
		frame: ContextSourceFrame<'_>,
	) -> SemanticContextRead {
		let result = match slot {
			Slot::ResidentSkills => Ok(Some(json!(
				frame
					.prepared
					.iter()
					.map(|block| block.text.as_str())
					.collect::<String>()
			))),
			Slot::RegistrySkills => {
				async {
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
				.await
			}
			Slot::References => self.documents(frame.entry).await.map(Some),
			Slot::Skills => self.skill_context(run).await.map(|text| Some(json!(text))),
			Slot::Observation => self
				.home(run)
				.observation(0, aidash_domain::context::observation::DEFAULT_LIMIT)
				.await
				.map(Some),
			Slot::Semantic => {
				return self
					.semantic_context(
						run,
						frame.task,
						frame.inputs,
						frame.budget,
						frame.entry,
						frame.key,
						frame.degrade_on_outage,
					)
					.await;
			}
		};
		SemanticContextRead {
			selections: vec![],
			contributions: vec![],
			workspace_retrieved: false,
			result,
		}
	}
}
