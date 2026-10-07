//! Exact definition admission and Node operation dispatch share portable contracts.
use crate::Result;
use aidash_domain::{
	Run,
	registry::{
		Entry, Projection,
		bindings::{QualifiedRef, ResolvedBinding},
	},
	tool::{ToolContract, providers::ToolDescriptor},
};
use async_trait::async_trait;
use std::sync::Arc;

/// Implementations retain the caller's current catalog/subject authority scope.
#[async_trait]
pub trait BindingCatalog: Send {
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry>;
	async fn foreign_agent(
		&mut self,
		_: &QualifiedRef,
	) -> Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		Err(crate::Error::Forbidden)
	}
	/// New execution admission requires this revision to be active and approved.
	/// A structural staging preview may check a readable pending revision, but
	/// that preview cannot authorize activation or execution.
	async fn installation(&mut self, projection: &Projection) -> Result<()>;
	async fn source(&mut self, definition: &Entry) -> Result<()>;
}

/// The Node admits implementations and their factual contracts, independently of publishers.
pub trait ProviderCatalog: Send + Sync {
	fn contract(
		&self,
		descriptor: &ToolDescriptor,
		identity: &QualifiedRef,
	) -> Result<ToolContract>;
	fn implementation(&self, descriptor: &ToolDescriptor) -> Result<String>;
}

#[async_trait]
pub trait ProviderSet: ProviderCatalog {
	/// Rechecks availability and binds the exact snapshot; it never selects a new definition.
	async fn bind(
		&self,
		run: &Run,
		binding: &ResolvedBinding,
	) -> Result<Arc<dyn super::execution::ExecutionTool>>;
}

#[async_trait]
pub trait BindingResolver: Send + Sync {
	/// Runtime reconstruction is from a previously persisted complete snapshot.
	async fn tools(&self, run: &Run) -> Result<super::execution::Tools>;
}

/// Resource policy and retained installation approval are evaluated live. An
/// active pointer is required at admission, but never replaces a retained Run
/// revision or revokes it by itself.
#[async_trait]
pub trait BindingAuthority: Send + Sync {
	/// Acquire current authority once per assembly or invocation boundary.
	async fn refresh(&self, _run: &Run) -> Result<()> {
		Ok(())
	}
	async fn check(&self, run: &Run, binding: &ResolvedBinding) -> Result<()>;
}
