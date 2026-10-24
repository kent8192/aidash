//! Raw storage rows and the narrow metadata used by inspection and delivery.
use super::*;
use crate::{context::Context, entities::RunControl};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunMetadata {
	pub id: Uuid,
	pub task_id: Uuid,
	pub workspace_id: Uuid,
	pub node: crate::nodes::NodeName,
	pub home_node: String,
	pub agent_id: String,
	pub agent_version: String,
	pub phase: RunPhase,
	pub control: RunControl,
	pub step: i32,
	pub revision: i64,
	pub observed_input_seq: i64,
	pub ledger_worker_ready: bool,
	pub error: Option<String>,
	pub lease_owner: Option<Uuid>,
	pub lease_until: Option<DateTime<Utc>>,
	pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRun {
	#[serde(flatten)]
	pub metadata: RunMetadata,
	pub context: Value,
	pub pending: Value,
}
impl RawRun {
	pub fn decode(&self) -> Result<Run> {
		let (state, recovery) = decode(self.metadata.phase, self.pending.clone())?;
		let context = serde_json::from_value::<Context>(self.context.clone())
			.map_err(|_| Error::Invalid("invalid execution context".into()))?;
		let m = &self.metadata;
		Ok(Run {
			id: m.id,
			task_id: m.task_id,
			workspace_id: m.workspace_id,
			node: m.node.clone(),
			home_node: m.home_node.clone(),
			agent_id: m.agent_id.clone(),
			agent_version: m.agent_version.clone(),
			state_version: StateVersion::default(),
			state,
			recovery,
			control: m.control,
			context,
			step: m.step,
			revision: m.revision,
			observed_input_seq: m.observed_input_seq,
			ledger_worker_ready: m.ledger_worker_ready,
			error: m.error.clone(),
			lease_owner: m.lease_owner,
			lease_until: m.lease_until,
			updated_at: m.updated_at,
		})
	}
	pub fn inspect(self) -> RunInspection {
		let lease_wait = StateRequirements::from_pending(&self.pending)
			.ok()
			.and_then(|requires| self.metadata.lease_wait(&requires));
		match self.decode() {
			Ok(run) => RunInspection {
				state_version: StateVersion::default(),
				metadata: self.metadata,
				state: Some(run.state),
				recovery: Some(run.recovery),
				context: Some(run.context),
				state_error: None,
				lease_wait,
			},
			Err(error) => {
				// A validated failure disposition is inspectable without making its
				// diagnostic Context executable or inventing a replacement Context.
				let delivery = decode(self.metadata.phase, self.pending)
					.ok()
					.filter(|(state, _)| state.failure_delivery());
				let (state, recovery) = match delivery {
					Some((state, recovery)) => (Some(state), Some(recovery)),
					None => (None, None),
				};
				RunInspection {
					state_version: StateVersion::default(),
					metadata: self.metadata,
					state,
					recovery,
					context: None,
					state_error: Some(error.to_string()),
					lease_wait,
				}
			}
		}
	}
}

impl Run {
	pub fn metadata(&self) -> RunMetadata {
		RunMetadata {
			id: self.id,
			task_id: self.task_id,
			workspace_id: self.workspace_id,
			node: self.node.clone(),
			home_node: self.home_node.clone(),
			agent_id: self.agent_id.clone(),
			agent_version: self.agent_version.clone(),
			phase: self.phase(),
			control: self.control,
			step: self.step,
			revision: self.revision,
			observed_input_seq: self.observed_input_seq,
			ledger_worker_ready: self.ledger_worker_ready,
			error: self.error.clone(),
			lease_owner: self.lease_owner,
			lease_until: self.lease_until,
			updated_at: self.updated_at,
		}
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunInspection {
	pub state_version: StateVersion,
	#[serde(flatten)]
	pub metadata: RunMetadata,
	pub state: Option<RunState>,
	pub recovery: Option<RecoveryState>,
	#[serde(serialize_with = "crate::context::serialize_inspection_context")]
	#[schemars(with = "Option<crate::context::InspectionContext<'static>>")]
	pub context: Option<Context>,
	pub state_error: Option<String>,
	/// Set while only a worker with the listed capabilities may claim the Run.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lease_wait: Option<LeaseWait>,
}
impl std::ops::Deref for RunInspection {
	type Target = RunMetadata;
	fn deref(&self) -> &Self::Target {
		&self.metadata
	}
}
impl From<Run> for RunInspection {
	fn from(run: Run) -> Self {
		Self {
			state_version: StateVersion::default(),
			metadata: run.metadata(),
			state: Some(run.state),
			recovery: Some(run.recovery),
			context: Some(run.context),
			state_error: None,
			lease_wait: None,
		}
	}
}

impl RunMetadata {
	/// Whether the Run's Home is its own executing Node in `deployment`.
	pub fn is_local(&self, deployment: &str) -> bool {
		crate::nodes::is_home(&self.home_node, deployment, self.node.as_str())
	}

	/// The subject of the Run's agent on its executing Node in `deployment`.
	pub fn executor_subject(&self, deployment: &str) -> String {
		crate::entities::agent_subject(
			&format!("{deployment}/{}", self.node),
			&self.agent_id,
			&self.agent_version,
		)
	}
}

impl From<&Run> for RunMetadata {
	fn from(run: &Run) -> Self {
		run.metadata()
	}
}
impl From<&RunMetadata> for RunMetadata {
	fn from(run: &RunMetadata) -> Self {
		run.clone()
	}
}
impl From<&RunInspection> for RunMetadata {
	fn from(run: &RunInspection) -> Self {
		run.metadata.clone()
	}
}
impl std::ops::Deref for RawRun {
	type Target = RunMetadata;
	fn deref(&self) -> &Self::Target {
		&self.metadata
	}
}

impl RunMetadata {
	pub fn phase(&self) -> RunPhase {
		self.phase
	}
	/// A runnable Run that needs capabilities is waiting for a capable worker
	/// whenever no live lease holds it; this is not a stuck or damaged Run. A
	/// cancelled Run is not waiting: any worker finishes its cancellation.
	pub fn lease_wait(&self, requires: &StateRequirements) -> Option<LeaseWait> {
		let held = self.lease_until.is_some_and(|until| until > Utc::now());
		(!requires.is_empty()
			&& !held && !self.phase.is_terminal()
			&& self.control == RunControl::Active)
			.then(|| LeaseWait::CapableWorker {
				capabilities: requires.names().to_vec(),
			})
	}
}

impl RawRun {
	/// Manual retry of a semantic or context pause clears its typed reason and
	/// retry schedule while retaining the execution state.
	pub fn resumed_pending(&self) -> crate::Result<Option<Value>> {
		let (state, mut recovery) = decode(self.phase, self.pending.clone())?;
		if !state.failure_delivery() {
			self.decode()?;
		}
		if self.control == RunControl::Paused
			&& (recovery.semantic_reason.is_some() || recovery.context_reason.is_some())
		{
			recovery.retry = None;
			recovery.semantic_reason = None;
			recovery.context_reason = None;
			recovery.decision_cycle = recovery.decision_cycle.saturating_add(1);
			recovery.decider_retry = 0;
			Ok(Some(encode(&state, &recovery)?))
		} else {
			Ok(None)
		}
	}
}
impl RawRun {
	/// The semantic pause an authorized manual retry of this Run clears, if
	/// any. Clearing one opens the Run's next Retry Cycle (ADR 0057); a context
	/// pause or an ordinary pause does not.
	pub fn manual_semantic_retry(&self) -> crate::Result<Option<crate::semantic::Failure>> {
		let (_, recovery) = decode(self.phase, self.pending.clone())?;
		Ok(recovery
			.semantic_reason
			.filter(|_| self.control == RunControl::Paused))
	}
}
#[cfg(test)]
mod resume_tests;
