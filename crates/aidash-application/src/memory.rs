//! Native retain/recall/derived-memory/reflect orchestration adapted from Hindsight.
use crate::{Error, Result, ports::memory::*};
use aidash_domain::{memory::*, registry::EntityRef};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

struct Budget(Allowance);
impl Budget {
	fn new(bounds: &Bounds) -> Self {
		Self(Allowance {
			calls: bounds.max_model_calls,
			tokens: bounds.max_model_tokens,
			cost_micros: bounds.max_cost_micros,
		})
	}
	fn allowance(&self) -> Result<Allowance> {
		if self.0.calls == 0 || self.0.tokens == 0 || self.0.cost_micros == 0 {
			return Err(Error::Invalid("memory model budget exhausted".into()));
		}
		Ok(self.0)
	}
	fn charge(&mut self, usage: Usage) -> Result<()> {
		self.0.calls = self
			.0
			.calls
			.checked_sub(1)
			.ok_or_else(|| Error::Invalid("memory model call budget exceeded".into()))?;
		self.0.tokens = self
			.0
			.tokens
			.checked_sub(usage.tokens)
			.ok_or_else(|| Error::Invalid("memory model token budget exceeded".into()))?;
		self.0.cost_micros = self
			.0
			.cost_micros
			.checked_sub(usage.cost_micros)
			.ok_or_else(|| Error::Invalid("memory model cost budget exceeded".into()))?;
		Ok(())
	}
}

pub struct Engine<'a> {
	pub provider: &'a EntityRef,
	pub policy: &'a Policy,
	pub models: &'a dyn MemoryModels,
}
pub struct Consolidated {
	pub units: Vec<Unit>,
	allowance: Allowance,
}
impl Engine<'_> {
	fn validate(&self) -> Result<()> {
		self.policy.validate().map_err(Into::into)
	}
	fn query(&self, query: &RecallQuery) -> Result<()> {
		if query.text.trim().is_empty()
			|| query.text.len() > self.policy.bounds.max_input_bytes
			|| query.max_tokens > self.policy.bounds.max_context_tokens
			|| query
				.time
				.as_ref()
				.is_some_and(|time| time.start > time.end)
		{
			return Err(Error::Invalid("invalid bounded memory query".into()));
		}
		Ok(())
	}
	pub async fn mutate(
		&self,
		scope: &mut dyn MemoryScope,
		mutation: &Mutation,
	) -> Result<Vec<Unit>> {
		self.validate()?;
		mutation.validate(self.policy)?;
		if &mutation.provider != self.provider {
			return Err(Error::Conflict("memory provider binding changed".into()));
		}
		scope
			.authorize(&mutation.bank, self.provider, "memory.write")
			.await?;
		scope.mutate(mutation, &self.policy.bounds).await
	}

	async fn extract(
		&self,
		text: &str,
		evidence: &[Evidence],
		mode: extraction::Mode,
		budget: &mut Budget,
	) -> Result<extraction::Extraction> {
		if text.trim().is_empty()
			|| text.len() > self.policy.bounds.max_input_bytes
			|| evidence.is_empty()
			|| evidence.len() > self.policy.bounds.max_evidence
		{
			return Err(Error::Invalid("invalid memory extraction input".into()));
		}
		let output = self
			.models
			.extract(
				&self.policy.extraction,
				text,
				evidence,
				mode,
				&self.policy.bounds,
				budget.allowance()?,
			)
			.await?;
		budget.charge(output.usage)?;
		if output.output.facts.len() > self.policy.bounds.max_candidates {
			return Err(Error::Invalid("extractor exceeded candidate limit".into()));
		}
		for content in &output.output.facts {
			content.validate(&self.policy.bounds)?;
			if content.kind.derived()
				|| content.verification != Verification::Unverified
				|| content.evidence.is_empty()
				|| content.evidence.iter().any(|e| !evidence.contains(e))
				|| content.links.iter().any(|link| {
					!evidence.iter().any(|proof| {
						matches!(proof,
					Evidence::Unit { id, revision, .. } if *id == link.target && *revision == link.revision)
					})
				}) {
				return Err(Error::Invalid(
					"extractor invented evidence or verification".into(),
				));
			}
		}
		Ok(output.output)
	}

	/// Caller explicitly requested admission. Run learning uses `learn` and remains a candidate.
	pub async fn retain(
		&self,
		scope: &mut dyn MemoryScope,
		bank: &Bank,
		operation_id: Uuid,
		text: &str,
		evidence: &[Evidence],
	) -> Result<Vec<Unit>> {
		self.validate()?;
		scope.authorize(bank, self.provider, "memory.write").await?;
		scope.current(bank, evidence).await?;
		let extracted = self
			.extract(
				text,
				evidence,
				extraction::Mode::Retain,
				&mut Budget::new(&self.policy.bounds),
			)
			.await?;
		let ids: Vec<_> = (0..extracted.facts.len())
			.map(|index| operation_unit(operation_id, index))
			.collect();
		let content = extracted.resolve(&ids, &self.policy.bounds)?;
		if content.is_empty() {
			return Ok(vec![]);
		}
		let mutation = Mutation {
			operation_id,
			provider: self.provider.clone(),
			bank: bank.clone(),
			changes: content
				.into_iter()
				.enumerate()
				.map(|(index, content)| Change::Add {
					id: operation_unit(operation_id, index),
					content,
				})
				.collect(),
		};
		// The adapter locks and rechecks all evidence after model I/O.
		self.mutate(scope, &mutation).await
	}

	pub async fn learn(
		&self,
		scope: &mut dyn MemoryScope,
		bank: &Bank,
		run: &Evidence,
		text: &str,
		evidence: &[Evidence],
	) -> Result<Vec<Candidate>> {
		self.validate()?;
		if !self.policy.learn_from_runs {
			return Err(Error::Forbidden);
		}
		if !matches!(run, Evidence::Run { .. }) || !evidence.contains(run) {
			return Err(Error::Invalid(
				"Run learning needs durable complete Run evidence".into(),
			));
		}
		scope
			.authorize(bank, self.provider, "memory.candidate.propose")
			.await?;
		scope.current(bank, evidence).await?;
		let extracted = self
			.extract(
				text,
				evidence,
				extraction::Mode::RunCandidate,
				&mut Budget::new(&self.policy.bounds),
			)
			.await?;
		if !extracted.causal.is_empty() {
			return Err(Error::Invalid(
				"Run candidates cannot bind causal links to unadmitted batch identities".into(),
			));
		}
		scope
			.propose(bank, run, &extracted.facts, &self.policy.bounds)
			.await
	}

	/// Reconcile semantic neighbors against complete mandatory support. Model
	/// selection cannot remove a conflict or introduce a foreign revision.
	pub async fn consolidate(
		&self,
		scope: &mut dyn MemoryScope,
		trigger: &Unit,
		snapshot: &[Unit],
	) -> Result<Consolidated> {
		self.validate()?;
		scope
			.authorize(&trigger.bank, self.provider, "memory.derive")
			.await?;
		let initial = consolidation::sources(trigger, snapshot, &self.policy.bounds)?;
		let mandatory =
			consolidation::preserve_observations(&initial, snapshot, &self.policy.bounds)?;
		let known: BTreeMap<_, _> = snapshot
			.iter()
			.filter(|unit| {
				unit.visible()
					&& !unit.content.kind.derived()
					&& unit.content.kind == trigger.content.kind
					&& unit.content.learning == trigger.content.learning
					&& !mandatory.iter().any(|base| base.id == unit.id)
			})
			.map(|unit| (unit.id, unit))
			.collect();
		let mut budget = Budget::new(&self.policy.bounds);
		if known.is_empty() {
			return Ok(Consolidated {
				units: mandatory,
				allowance: budget.allowance()?,
			});
		}
		let semantic = scope
			.semantic(
				&trigger.bank,
				&self.policy.embedding,
				&trigger.content.text,
				&known.keys().copied().collect::<Vec<_>>(),
				self.policy.bounds.max_candidates,
				budget.allowance()?,
			)
			.await?;
		budget.charge(semantic.usage)?;
		if semantic.output.len() > self.policy.bounds.max_candidates
			|| semantic.output.iter().any(|id| !known.contains_key(id))
		{
			return Err(Error::Invalid(
				"consolidation retrieval escaped its authorized candidates".into(),
			));
		}
		let candidates: Vec<_> = semantic
			.output
			.iter()
			.collect::<BTreeSet<_>>()
			.into_iter()
			.map(|id| known[id].clone())
			.collect();
		if candidates.is_empty() {
			return Ok(Consolidated {
				units: mandatory,
				allowance: budget.allowance()?,
			});
		}
		let selection = self
			.models
			.consolidate(
				&self.policy.derivation,
				&mandatory,
				&candidates,
				&self.policy.bounds,
				budget.allowance()?,
			)
			.await?;
		budget.charge(selection.usage)?;
		let selected = consolidation::select(
			&mandatory,
			&candidates,
			&selection.output,
			&self.policy.bounds,
		)?;
		let units = consolidation::preserve_observations(&selected, snapshot, &self.policy.bounds)?;
		scope
			.current(
				&trigger.bank,
				&units.iter().map(Unit::evidence).collect::<Vec<_>>(),
			)
			.await?;
		Ok(Consolidated {
			units,
			allowance: budget.allowance()?,
		})
	}

	pub async fn maintain_consolidated(
		&self,
		scope: &mut dyn MemoryScope,
		mutation: &Mutation,
		sources: &Consolidated,
	) -> Result<Vec<Unit>> {
		self.maintain_with_allowance(
			scope,
			mutation,
			Kind::Observation,
			&sources.units,
			sources.allowance,
		)
		.await
	}

	/// Maintenance admits no new Run knowledge and cannot rewrite its source units.
	pub async fn maintain(
		&self,
		scope: &mut dyn MemoryScope,
		mutation: &Mutation,
		kind: Kind,
		sources: &[Unit],
	) -> Result<Vec<Unit>> {
		self.maintain_with_allowance(
			scope,
			mutation,
			kind,
			sources,
			Budget::new(&self.policy.bounds).allowance()?,
		)
		.await
	}
	async fn maintain_with_allowance(
		&self,
		scope: &mut dyn MemoryScope,
		mutation: &Mutation,
		kind: Kind,
		sources: &[Unit],
		allowance: Allowance,
	) -> Result<Vec<Unit>> {
		self.validate()?;
		if !kind.derived()
			|| sources.is_empty()
			|| sources.len() > self.policy.bounds.max_candidates
			|| sources
				.iter()
				.any(|unit| !unit.visible() || unit.bank != mutation.bank)
			|| mutation.changes.len() != 1
			|| matches!(mutation.changes[0], Change::Delete { .. })
		{
			return Err(Error::Invalid("invalid derived memory maintenance".into()));
		}
		let mental_model = match &mutation.changes[0] {
			Change::Add { content, .. } | Change::Correct { content, .. } => {
				content.mental_model.as_ref()
			}
			Change::Delete { .. } => unreachable!(),
		};
		if (kind == Kind::MentalModel) != mental_model.is_some() {
			return Err(Error::Invalid(
				"mental model maintenance requires its recurring question".into(),
			));
		}
		let evidence: Vec<_> = sources.iter().map(Unit::evidence).collect();
		scope
			.authorize(&mutation.bank, self.provider, "memory.derive")
			.await?;
		scope.current(&mutation.bank, &evidence).await?;
		let mut budget = Budget(allowance);
		let result = self
			.models
			.derive(
				&self.policy.derivation,
				kind,
				mental_model,
				sources,
				&self.policy.bounds,
				budget.allowance()?,
			)
			.await?;
		budget.charge(result.usage)?;
		let content = result.output;
		content.validate(&self.policy.bounds)?;
		if content.kind != kind
			|| content.mental_model.as_ref() != mental_model
			|| content.verification != Verification::Unverified
			|| content.evidence.is_empty()
			|| content.evidence.iter().any(|e| !evidence.contains(e))
			|| (kind == Kind::Observation
				&& content.evidence.iter().collect::<BTreeSet<_>>()
					!= evidence.iter().collect::<BTreeSet<_>>())
		{
			return Err(Error::Invalid("derived memory invented support".into()));
		}
		let mut publication = mutation.clone();
		match &mut publication.changes[0] {
			Change::Add {
				content: target, ..
			}
			| Change::Correct {
				content: target, ..
			} => *target = content,
			Change::Delete { .. } => unreachable!("checked above"),
		}
		self.mutate(scope, &publication).await
	}

	pub async fn recall(
		&self,
		scope: &mut dyn MemoryScope,
		bank: &Bank,
		query: &RecallQuery,
	) -> Result<Recall> {
		self.recall_in(scope, bank, query, &mut Budget::new(&self.policy.bounds))
			.await
	}
	async fn recall_in(
		&self,
		scope: &mut dyn MemoryScope,
		bank: &Bank,
		query: &RecallQuery,
		budget: &mut Budget,
	) -> Result<Recall> {
		self.validate()?;
		self.query(query)?;
		scope.authorize(bank, self.provider, "memory.read").await?;
		if query.max_tokens == 0 {
			return Ok(Recall::NoSpace);
		}
		let snapshot = scope.snapshot(bank, self.policy.bounds.max_units).await?;
		if snapshot.units.len() > self.policy.bounds.max_units {
			return Err(Error::Invalid(
				"memory snapshot exceeded source limit".into(),
			));
		}
		let units: Vec<_> = snapshot
			.units
			.iter()
			.filter(|u| {
				u.visible() && (query.kinds.is_empty() || query.kinds.contains(&u.content.kind))
			})
			.cloned()
			.collect();
		let allowed: BTreeSet<_> = units.iter().map(|u| u.id).collect();
		if allowed.len() != units.len() || units.iter().any(|u| &u.bank != bank) {
			return Err(Error::Invalid("invalid memory snapshot".into()));
		}
		if units.is_empty() {
			scope
				.deliver(bank, &snapshot.authority_revision, &[])
				.await?;
			return Ok(Recall::Empty);
		}
		let ids: Vec<_> = allowed.iter().copied().collect();
		let limit = self.policy.bounds.max_candidates;
		let semantic = scope
			.semantic(
				bank,
				&self.policy.embedding,
				&query.text,
				&ids,
				limit,
				budget.allowance()?,
			)
			.await?;
		budget.charge(semantic.usage)?;
		let semantic = semantic.output;
		let keyword = scope.keyword(bank, &query.text, &ids, limit).await?;
		if semantic.len() > limit || keyword.len() > limit {
			return Err(Error::Invalid(
				"memory search exceeded candidate limit".into(),
			));
		}
		let graph =
			recall::graph_with_edges(&units, &semantic, &snapshot.graph, &self.policy.bounds);
		let temporal = recall::temporal(&units, query.time.as_ref(), limit);
		let fused = recall::fuse(
			&recall::Rankings {
				semantic,
				keyword,
				graph,
				temporal,
			},
			&allowed,
			limit,
		)?;
		let by_id: BTreeMap<_, _> = units.into_iter().map(|unit| (unit.id, unit)).collect();
		let candidates: Vec<_> = fused
			.iter()
			.map(|ranked| by_id[&ranked.id].clone())
			.collect();
		if candidates.is_empty() {
			scope
				.deliver(bank, &snapshot.authority_revision, &[])
				.await?;
			return Ok(Recall::Empty);
		}
		let ranked = self
			.models
			.rerank(
				&self.policy.reranker,
				&query.text,
				&candidates,
				budget.allowance()?,
			)
			.await?;
		budget.charge(ranked.usage)?;
		let mut scores = ranked.output;
		let ranked_ids: BTreeSet<_> = scores.iter().map(|(id, _)| *id).collect();
		if scores.len() != candidates.len()
			|| ranked_ids.len() != scores.len()
			|| ranked_ids != candidates.iter().map(|unit| unit.id).collect()
			|| scores.iter().any(|(_, score)| !score.is_finite())
		{
			return Err(Error::Invalid(
				"reranker returned an invalid candidate permutation".into(),
			));
		}
		scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
		let mut selected = Vec::new();
		for (id, _) in scores.into_iter().take(self.policy.bounds.max_results) {
			let mut proposed = selected.clone();
			proposed.push(by_id[&id].clone());
			let envelope = serde_json::to_string(&Recall::Ready {
				units: proposed.clone(),
			})?;
			if self
				.models
				.tokens(&self.policy.tokenizer, &envelope)
				.await? <= query.max_tokens
			{
				selected = proposed;
			}
		}
		if selected.is_empty() {
			scope
				.deliver(bank, &snapshot.authority_revision, &[])
				.await?;
			return Ok(Recall::NoSpace);
		}
		let evidence: Vec<_> = selected
			.iter()
			.map(Unit::evidence)
			.chain(selected.iter().flat_map(|u| u.content.evidence.clone()))
			.collect();
		scope
			.deliver(bank, &snapshot.authority_revision, &evidence)
			.await?;
		Ok(Recall::Ready { units: selected })
	}

	/// Reflection is explicitly invoked and reads admitted, currently authorized memory only.
	pub async fn reflect(
		&self,
		scope: &mut dyn MemoryScope,
		bank: &Bank,
		query: &RecallQuery,
	) -> Result<Reflection> {
		self.validate()?;
		self.query(query)?;
		scope
			.authorize(bank, self.provider, "memory.reflect")
			.await?;
		let mut budget = Budget::new(&self.policy.bounds);
		let mut context = Vec::<Unit>::new();
		let initial = self.recall_in(scope, bank, query, &mut budget).await?;
		if let Recall::Ready { units } = initial {
			context = units;
		}
		loop {
			let evidence: Vec<_> = context.iter().map(Unit::evidence).collect();
			scope.current(bank, &evidence).await?;
			let output = self
				.models
				.reflect(
					&self.policy.reflection,
					&query.text,
					&context,
					&self.policy.bounds,
					budget.allowance()?,
				)
				.await?;
			budget.charge(output.usage)?;
			match output.output {
				ReflectStep::Answer { reflection } => {
					if reflection.text.trim().is_empty()
						|| reflection.evidence.is_empty()
						|| reflection.evidence.len() > self.policy.bounds.max_evidence
						|| reflection.evidence.iter().any(|e| !evidence.contains(e))
						|| self
							.models
							.tokens(&self.policy.tokenizer, &serde_json::to_string(&reflection)?)
							.await? > query.max_tokens
					{
						return Err(Error::Invalid(
							"reflection lacks current citations or exceeds budget".into(),
						));
					}
					scope
						.authorize(bank, self.provider, "memory.reflect")
						.await?;
					scope.current(bank, &reflection.evidence).await?;
					return Ok(reflection);
				}
				ReflectStep::Recall { query: followup } => {
					if followup.max_tokens > query.max_tokens {
						return Err(Error::Invalid("reflection context budget exhausted".into()));
					}
					let recall = self.recall_in(scope, bank, &followup, &mut budget).await?;
					if let Recall::Ready { units } = recall {
						context = units;
					}
				}
				ReflectStep::Read { id, revision } => {
					let snapshot = scope.snapshot(bank, self.policy.bounds.max_units).await?;
					let unit = snapshot
						.units
						.into_iter()
						.find(|u| {
							u.id == id && u.revision == revision && u.visible() && &u.bank == bank
						})
						.ok_or_else(|| Error::Conflict("reflection source changed".into()))?;
					let mut proposed = context.clone();
					proposed.retain(|u| u.id != unit.id);
					proposed.push(unit);
					if proposed.len() > self.policy.bounds.max_results
						|| self
							.models
							.tokens(
								&self.policy.tokenizer,
								&serde_json::to_string(&Recall::Ready {
									units: proposed.clone(),
								})?,
							)
							.await? > query.max_tokens
					{
						return Err(Error::Invalid("reflection context budget exhausted".into()));
					}
					let evidence: Vec<_> = proposed.iter().map(Unit::evidence).collect();
					scope
						.deliver(bank, &snapshot.authority_revision, &evidence)
						.await?;
					context = proposed;
				}
			}
		}
	}
}

/// Stable unit identities across extraction retries, without UUID-v5 dependencies.
fn operation_unit(operation: Uuid, index: usize) -> Uuid {
	use sha2::{Digest, Sha256};
	let mut hash = Sha256::new();
	hash.update(b"aidash-memory-unit-v1");
	hash.update(operation.as_bytes());
	hash.update((index as u64).to_be_bytes());
	let digest = hash.finalize();
	let mut bytes = [0u8; 16];
	bytes.copy_from_slice(&digest[..16]);
	bytes[6] = (bytes[6] & 0x0f) | 0x80;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests;
