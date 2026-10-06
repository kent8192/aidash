//! Patch storage uses the same verified file and publication scope as ordinary file tools.
use super::files::Scope;
use crate::apps::execution::capabilities::serializers::contracts::{
	FileEntry, Patch as NativePatch,
};
use aidash_application::{Error, Result, ports::capabilities::patch::PatchScope};
use aidash_domain::capabilities::{operations::MountedFile, patch::Patch};
use async_trait::async_trait;
impl From<NativePatch> for Patch {
	fn from(v: NativePatch) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			expected_revision: v.expected_revision,
			preconditions: v.preconditions,
			patch: v.patch,
		}
	}
}
#[async_trait]
impl PatchScope for Scope<'_> {
	fn patch_limits(&self) -> Result<(usize, bool)> {
		let p = &self
			.store
			.ok_or_else(|| Error::External("patch repository scope invariant".into()))?
			.capabilities
			.0;
		Ok((p.limits.patch_bytes, p.admission))
	}
	async fn read_file(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		let file: FileEntry = file.clone().into();
		self.store
			.ok_or_else(|| Error::External("patch repository scope invariant".into()))?
			.capabilities
			.read(self.access, &file)
			.await
			.map_err(Into::into)
	}
}
