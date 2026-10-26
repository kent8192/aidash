//! Lifecycle execution of resolved context handles. All native classification,
//! provider dispatch and output keys are owned by the environment's plan.
use super::*;

impl Executor<'_> {
	pub(super) async fn ordered_instruction_source(
		&self,
		run: &mut Run,
		token: Uuid,
		bindings: &str,
		source: &PlannedContextSource,
		frame: ContextSourceFrame<'_>,
	) -> Result<(String, OrderedSkill)> {
		let revision = self
			.environment
			.source_revision(&source.handle, run)
			.await?;
		let cached = match (revision, &run.context.source_observation) {
			(Some(revision), Some(observation)) => {
				observation.skill_at(revision, bindings)?.map(str::to_owned)
			}
			_ => None,
		};
		if let Some(text) = cached.filter(|_| !source.refresh_each_step) {
			match self
				.environment
				.recheck_source_observation(run, &json!({}))
				.await
			{
				Ok(()) => return Ok((text, OrderedSkill::Reused)),
				Err(Error::Conflict(_)) => {}
				Err(error) => return Err(error),
			}
		}
		let text = read_planned_source(self.environment, run, token, source, frame)
			.await?
			.and_then(|value| value.as_str().map(str::to_owned))
			.unwrap_or_default();
		let observation = revision
			.map(|revision| context::sources::SkillObservation::new(revision, text.clone()))
			.transpose()?;
		Ok((text, OrderedSkill::Retrieved(observation)))
	}

	pub(super) async fn ordered_data_sources(
		&self,
		run: &mut Run,
		token: Uuid,
		plan: &[PlannedContextSource],
		bindings: &str,
		retrieves: bool,
		frame: ContextSourceFrame<'_>,
	) -> Result<OrderedSemantic> {
		let has_sources = plan
			.iter()
			.any(|source| source.phase == ContextSourcePhase::BudgetedData);
		let key = if retrieves && has_sources {
			Some(
				self.environment
					.source_retrieval_key(run, bindings, frame)
					.await?,
			)
		} else {
			None
		};
		let boundary = key.as_ref().map_or_else(
			|| format!("{}:no-budgeted-sources", run.id),
			|key| key.digest(),
		);
		let cached = run
			.context
			.source_observation
			.as_ref()
			.map(|observation| observation.at(&boundary, bindings))
			.transpose()?
			.flatten()
			.cloned();
		let reusable = plan
			.iter()
			.filter(|source| source.phase == ContextSourcePhase::BudgetedData)
			.all(|source| {
				source.cache == ContextSourceCache::Retrieval && !source.refresh_each_step
			});
		if let Some(content) = cached.filter(|_| reusable) {
			match self
				.environment
				.recheck_source_observation(run, &content)
				.await
			{
				Ok(()) => {
					return Ok(OrderedSemantic {
						values: cached_data_phase(plan, ContextSourcePhase::BudgetedData, &content),
						boundary,
						retrieved: false,
					});
				}
				Err(Error::Conflict(_)) => {}
				Err(error) => return Err(error),
			}
		}
		let values = if let Some(key) = &key {
			collect_data_phase(
				self.environment,
				run,
				token,
				plan,
				ContextSourcePhase::BudgetedData,
				ContextSourceFrame {
					key: Some(key),
					..frame
				},
			)
			.await?
		} else {
			serde_json::Map::new()
		};
		Ok(OrderedSemantic {
			values,
			boundary,
			retrieved: true,
		})
	}
}

pub(super) async fn read_planned_source(
	environment: &dyn ExecutionEnvironment,
	run: &mut Run,
	token: Uuid,
	source: &PlannedContextSource,
	frame: ContextSourceFrame<'_>,
) -> Result<Option<Value>> {
	let read = environment
		.context_source(run, &source.handle, token, frame)
		.await;
	finish_source_read(
		environment.store(),
		run,
		token,
		&source.contribution_slot,
		read,
	)
	.await
}

pub(super) async fn collect_data_phase(
	environment: &dyn ExecutionEnvironment,
	run: &mut Run,
	token: Uuid,
	plan: &[PlannedContextSource],
	phase: ContextSourcePhase,
	frame: ContextSourceFrame<'_>,
) -> Result<serde_json::Map<String, Value>> {
	let mut values = serde_json::Map::new();
	for source in plan.iter().filter(|source| source.phase == phase) {
		let ContextSourceTarget::Pinned(key) = &source.target else {
			return Err(Error::Invalid("data source targets instructions".into()));
		};
		if let Some(value) = read_planned_source(environment, run, token, source, frame).await?
			&& values.insert(key.clone(), value).is_some()
		{
			return Err(Error::Invalid(
				"context sources share an output target".into(),
			));
		}
	}
	Ok(values)
}

pub(super) fn cached_data_phase(
	plan: &[PlannedContextSource],
	phase: ContextSourcePhase,
	content: &Value,
) -> serde_json::Map<String, Value> {
	plan.iter()
		.filter(|source| source.phase == phase)
		.filter_map(|source| {
			let ContextSourceTarget::Pinned(key) = &source.target else {
				return None;
			};
			content
				.get(key)
				.filter(|value| !value.is_null())
				.map(|value| (key.clone(), value.clone()))
		})
		.collect()
}
