//! Raw storage rows and the narrow metadata used by inspection and delivery.
use super::*;
use crate::{context::Context, domain::RunControl};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunMetadata {
	pub id: Uuid,
	pub task_id: Uuid,
	pub workspace_id: Uuid,
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
crate::native_record!(RunMetadata {
	id,
	task_id,
	workspace_id,
	home_node,
	agent_id,
	agent_version,
	phase,
	control,
	step,
	revision,
	observed_input_seq,
	ledger_worker_ready,
	error,
	lease_owner,
	lease_until,
	updated_at
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RawRun {
	#[sqlx(flatten)]
	#[serde(flatten)]
	pub metadata: RunMetadata,
	pub context: Value,
	pub pending: Value,
}
crate::native_record!(RawRun {
	metadata,
	context,
	pending
});

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
		match self.decode() {
			Ok(run) => RunInspection {
				state_version: StateVersion::default(),
				metadata: self.metadata,
				state: Some(run.state),
				recovery: Some(run.recovery),
				context: Some(run.context),
				state_error: None,
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
				}
			}
		}
	}
}
impl crate::database::native::Decode for Run {
	fn decode(row: &crate::database::native::Row, _: &[&str]) -> crate::Result<Self> {
		<RawRun as crate::database::native::Decode>::decode(row, &[])?
			.decode()
			.map_err(|error| crate::Error::Invalid(error.to_string()))
	}
}

impl Run {
	pub fn metadata(&self) -> RunMetadata {
		RunMetadata {
			id: self.id,
			task_id: self.task_id,
			workspace_id: self.workspace_id,
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
	pub context: Option<Context>,
	pub state_error: Option<String>,
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
		}
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
}
