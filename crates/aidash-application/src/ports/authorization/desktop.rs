//! Persist broker proofs and token families atomically with identity authority.
use crate::{
	Result,
	authorization::desktop::{Exchange, Renewal, Revocation, Start, Started, Tokens},
};
use async_trait::async_trait;

#[async_trait]
pub trait DesktopProtocol: Send + Sync {
	async fn start(&self, input: Start) -> Result<Started>;
	async fn exchange(&self, input: Exchange) -> Result<Tokens>;
	async fn refresh(&self, input: Renewal) -> Result<Tokens>;
	async fn revoke(&self, input: Revocation) -> Result<()>;
}
