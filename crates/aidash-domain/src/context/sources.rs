//! Bounded source reads survive recovery of one inference boundary. An Ordered
//! Run keys its semantic read by a Retrieval Key and its Skill context by the
//! Skill record revision, so later steps reuse identical bytes.
use crate::{Error, Result, registry::rules::digest, semantic::InputRead};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

const MAX_OBSERVATION_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceObservation {
	pub boundary: String,
	pub binding_digest: String,
	pub content: Value,
	pub digest: String,
	/// Ordered only: the Skill context, cached apart from the semantic read.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub skill: Option<SkillObservation>,
}
impl SourceObservation {
	pub fn new(boundary: String, binding_digest: String, content: Value) -> Result<Self> {
		if serde_json::to_vec(&content)?.len() > MAX_OBSERVATION_BYTES {
			return Err(Error::Invalid("source observation exceeds 1 MiB".into()));
		}
		Ok(Self {
			boundary,
			binding_digest,
			digest: digest(&content),
			content,
			skill: None,
		})
	}
	pub fn at(&self, boundary: &str, bindings: &str) -> Result<Option<&Value>> {
		if self.digest != digest(&self.content)
			|| serde_json::to_vec(&self.content)?.len() > MAX_OBSERVATION_BYTES
		{
			return Err(Error::Invalid(
				"source observation integrity changed".into(),
			));
		}
		Ok((self.boundary == boundary && self.binding_digest == bindings).then_some(&self.content))
	}
	/// The cached Skill context for this Skill record revision and binding graph.
	pub fn skill_at(&self, revision: i64, bindings: &str) -> Result<Option<&str>> {
		let Some(skill) = &self.skill else {
			return Ok(None);
		};
		if skill.digest != digest(&json!(skill.context)) {
			return Err(Error::Invalid(
				"source observation integrity changed".into(),
			));
		}
		Ok(
			(skill.revision == revision && self.binding_digest == bindings)
				.then_some(skill.context.as_str()),
		)
	}
}

/// Skill context observed at one Skill record revision.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillObservation {
	pub revision: i64,
	pub context: String,
	pub digest: String,
}
impl SkillObservation {
	pub fn new(revision: i64, context: String) -> Result<Self> {
		if context.len() > MAX_OBSERVATION_BYTES {
			return Err(Error::Invalid("source observation exceeds 1 MiB".into()));
		}
		Ok(Self {
			revision,
			digest: digest(&json!(context)),
			context,
		})
	}
}

/// Authority scope and source revisions read at a step boundary. Remote Runs
/// carry their admission values and no Home policy revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalScope {
	pub tenant: String,
	pub subject: String,
	pub authorization_revision: Option<i64>,
	pub index_revision: Option<i64>,
	pub participant_revision: Option<i64>,
	/// Digest of the content the semantic read draws from: every live
	/// semantic entry's ID, revision, state, index revision and point, and the
	/// revision of each memory bank the Run recalls. Inserting, editing,
	/// deleting or (re)indexing an entry, or mutating a memory unit, changes
	/// it, so ordinary content changes force a fresh retrieval instead of a
	/// stale reuse. Remote Runs carry none (#191).
	pub corpus_digest: Option<String>,
}

/// Everything an Ordered semantic read depends on. Equal keys must yield the
/// same retrieval bytes, so the key never contains the Run step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalKey {
	pub run_id: Uuid,
	pub query_digest: String,
	pub inputs: Vec<InputRead>,
	pub budget: usize,
	pub binding_digest: String,
	pub scope: RetrievalScope,
}
impl RetrievalKey {
	pub fn new(
		run_id: Uuid,
		task: &crate::Task,
		inputs: Vec<InputRead>,
		budget: usize,
		binding_digest: String,
		scope: RetrievalScope,
	) -> Self {
		Self {
			run_id,
			query_digest: digest(&json!({"title":task.title,"description":task.description})),
			inputs,
			budget,
			binding_digest,
			scope,
		}
	}
	pub fn digest(&self) -> String {
		digest(&json!(self))
	}
}

/// The Run-fixed semantic budget of an Ordered request, in bytes. The caller
/// caps it by the request's remaining headroom, as Legacy does.
pub fn ordered_semantic_budget(window: usize, max_output_tokens: u32) -> usize {
	window.saturating_sub(max_output_tokens as usize) / 8
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	#[test]
	fn durable_roundtrip_recovers_only_the_same_inference_and_binding_graph() {
		let observed = SourceObservation::new(
			"4:9:3".into(),
			"bindings-A".into(),
			json!({"memory":{"fact":"observed"}}),
		)
		.unwrap();
		let recovered: SourceObservation =
			serde_json::from_slice(&serde_json::to_vec(&observed).unwrap()).unwrap();
		assert_eq!(
			recovered.at("4:9:3", "bindings-A").unwrap(),
			Some(&observed.content)
		);
		assert!(recovered.at("5:9:3", "bindings-A").unwrap().is_none());
		assert!(recovered.at("4:10:3", "bindings-A").unwrap().is_none());
		assert!(recovered.at("4:9:3", "bindings-B").unwrap().is_none());
	}
	#[test]
	fn corrupted_or_oversized_observation_cannot_reach_inference() {
		let mut observed = SourceObservation::new(
			"boundary".into(),
			"bindings".into(),
			json!({"memory":"before"}),
		)
		.unwrap();
		observed.content = json!({"memory":"after"});
		assert!(observed.at("boundary", "bindings").is_err());
		assert!(
			SourceObservation::new(
				"boundary".into(),
				"bindings".into(),
				json!("x".repeat(1_048_576))
			)
			.is_err()
		);
	}
	#[test]
	fn legacy_observation_keeps_its_serialized_shape() {
		let observed =
			SourceObservation::new("1:0:0".into(), "bindings".into(), json!({"memory":null}))
				.unwrap();
		let encoded = serde_json::to_value(&observed).unwrap();
		assert_eq!(
			encoded.as_object().unwrap().keys().collect::<Vec<_>>(),
			["binding_digest", "boundary", "content", "digest"]
		);
	}
	#[test]
	fn skill_context_is_reused_only_at_the_same_record_revision_and_bindings() {
		let mut observed =
			SourceObservation::new("key".into(), "bindings".into(), json!({})).unwrap();
		observed.skill = Some(SkillObservation::new(3, "Pinned Skills".into()).unwrap());
		let recovered: SourceObservation =
			serde_json::from_slice(&serde_json::to_vec(&observed).unwrap()).unwrap();
		assert_eq!(
			recovered.skill_at(3, "bindings").unwrap(),
			Some("Pinned Skills")
		);
		assert_eq!(recovered.skill_at(4, "bindings").unwrap(), None);
		assert_eq!(recovered.skill_at(3, "other").unwrap(), None);
		let mut corrupted = recovered;
		corrupted.skill.as_mut().unwrap().context = "Injected".into();
		assert!(corrupted.skill_at(3, "bindings").is_err());
	}
	fn task() -> crate::Task {
		serde_json::from_value(json!({"id":Uuid::nil(),"workspace_id":Uuid::nil(),"title":"Task","description":"Describe","status":"RUNNING","requirements":{},"owner":null,"created_by":"human","dependencies":[],"parent_id":null,"revision":1,"created_at":"2026-10-02T00:00:00Z"})).unwrap()
	}
	fn scope() -> RetrievalScope {
		RetrievalScope {
			tenant: "tenant-a".into(),
			subject: "alice".into(),
			authorization_revision: Some(1),
			index_revision: Some(2),
			participant_revision: Some(3),
			corpus_digest: Some("corpus".into()),
		}
	}
	#[test]
	fn retrieval_key_changes_with_every_component_but_not_with_task_progress() {
		let key = |task: &crate::Task, budget, scope| {
			RetrievalKey::new(Uuid::nil(), task, vec![], budget, "bindings".into(), scope).digest()
		};
		let base = key(&task(), 100, scope());
		let mut progressed = task();
		progressed.revision = 7;
		assert_eq!(key(&progressed, 100, scope()), base);
		let mut retitled = task();
		retitled.title = "Other".into();
		assert_ne!(key(&retitled, 100, scope()), base);
		assert_ne!(key(&task(), 101, scope()), base);
		for change in [
			|s: &mut RetrievalScope| s.tenant = "tenant-b".into(),
			|s: &mut RetrievalScope| s.subject = "bob".into(),
			|s: &mut RetrievalScope| s.authorization_revision = Some(9),
			|s: &mut RetrievalScope| s.index_revision = None,
			|s: &mut RetrievalScope| s.participant_revision = Some(4),
			|s: &mut RetrievalScope| s.corpus_digest = Some("changed".into()),
		] {
			let mut changed = scope();
			change(&mut changed);
			assert_ne!(key(&task(), 100, changed), base);
		}
		let other_run = RetrievalKey::new(
			Uuid::from_u128(1),
			&task(),
			vec![],
			100,
			"bindings".into(),
			scope(),
		);
		assert_ne!(other_run.digest(), base);
	}
}
