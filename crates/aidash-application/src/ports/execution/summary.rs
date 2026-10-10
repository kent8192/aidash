//! Summary Stage authorization keeps catalog, Home pins and allowance storage outside the use case.
use crate::Result;
use aidash_domain::{
	context::summary::SummaryProvider,
	registry::{EntityRef, Entry},
	semantic::remote::Binding,
};
use async_trait::async_trait;

#[async_trait]
pub trait SummaryAuthorization: Send + Sync {
	fn remote(&self) -> bool;
	/// The execution node that runs the summarizer.
	fn node_id(&self) -> &str;
	/// Reacquire current execution authority before any approval decision.
	async fn refresh(&self) -> Result<()>;
	/// Current Home-disclosed semantic binding of a remote Run.
	async fn remote_binding(&self) -> Result<Binding>;
	/// Current Tenant catalog entry for the exact model version and action.
	async fn catalog_entry(&self, model: &EntityRef, action: &str) -> Result<Entry>;
	/// Charge every generated ancestor's summary allowance before I/O.
	async fn charge_generated(
		&self,
		summarizer: &SummaryProvider,
		request_bytes: i64,
	) -> Result<()>;
}
