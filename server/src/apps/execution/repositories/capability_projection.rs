//! Operation row conversion and output reading remain native adapters.
use super::{files::Scope, operations::Operation};
use crate::apps::execution::capabilities::serializers::contracts::{
	FileEntry, OperationResult as NativeResult,
};
use aidash_application::{Error, Result, ports::capabilities::projection::OutputReader};
use aidash_domain::capabilities::operations::{
	MountedFile,
	projection::{OperationResult, OperationView},
};
use async_trait::async_trait;
impl Operation {
	pub(crate) fn view(&self) -> OperationView<'_> {
		OperationView {
			id: self.id,
			area_id: self.area_id,
			kind: &self.kind,
			state: &self.state,
			generation: self.generation,
			revision: self.revision,
			epoch: self.epoch,
			policy_revision: self.policy_revision,
			input: &self.input,
			result: &self.result,
		}
	}
}
impl From<OperationResult> for NativeResult {
	fn from(v: OperationResult) -> Self {
		Self {
			operation_id: v.operation_id,
			kind: v.kind,
			status: v.status,
			area_id: v.area_id,
			generation: v.generation,
			revision: v.revision,
			epoch: v.epoch,
			policy_revision: v.policy_revision,
			termination_confirmed: v.termination_confirmed,
			writer_frozen: v.writer_frozen,
			session_id: v.session_id,
			displays: v.displays.into_iter().map(Into::into).collect(),
			exit_code: v.exit_code,
			output: v.output,
			next_offset: v.next_offset,
			truncated: v.truncated,
			effects_may_have_occurred: v.effects_may_have_occurred,
			error: v.error.map(Into::into),
		}
	}
}
#[async_trait]
impl OutputReader for Scope<'_> {
	fn read_bytes(&self) -> Result<usize> {
		Ok(self
			.store
			.ok_or_else(|| Error::External("output reader scope invariant".into()))?
			.capabilities
			.0
			.limits
			.read_bytes)
	}
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		self.store
			.ok_or_else(|| Error::External("output reader scope invariant".into()))?
			.capabilities
			.read(self.access, &FileEntry::from(file.clone()))
			.await
			.map_err(Into::into)
	}
}
