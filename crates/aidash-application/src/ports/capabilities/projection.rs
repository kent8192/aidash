//! Output bytes use the same native verified object reader as execution inputs.
use crate::Result;
use aidash_domain::capabilities::operations::MountedFile;
use async_trait::async_trait;
#[async_trait]
pub trait OutputReader: Send {
	fn read_bytes(&self) -> Result<usize>;
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
}
