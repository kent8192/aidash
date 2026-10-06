//! Patch scopes add full immutable reads to the shared file publication transaction.
use super::files::FileScopePort;
use crate::Result;
use aidash_domain::capabilities::operations::MountedFile;
use async_trait::async_trait;
#[async_trait]
pub trait PatchScope: FileScopePort {
	fn patch_limits(&self) -> Result<(usize, bool)>;
	async fn read_file(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
}
