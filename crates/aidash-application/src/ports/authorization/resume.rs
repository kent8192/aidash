//! Reacquisition borrows current authority; technical retry classification belongs to its adapter.
use crate::{Error, Result};
use async_trait::async_trait;
use std::time::Duration;
#[async_trait]
pub trait WorkerResumeScope: Send {
	async fn refresh(&mut self) -> Result<()>;
	async fn guard(&mut self) -> Result<()>;
	async fn inference(&mut self) -> Result<()>;
	fn retryable(&self, error: &Error) -> bool;
	async fn discard_failed_refresh(&mut self);
}
#[async_trait]
pub trait WorkerResumeRepository: Send + Sync {
	fn remote(&self) -> bool;
	async fn refresh_remote(&self) -> Result<()>;
	async fn lease(&self) -> Result<Box<dyn WorkerResumeScope + '_>>;
	async fn wait(&self, delay: Duration);
}
