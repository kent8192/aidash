//! Process composition and error conversion around the compaction adapter.
use crate::Result;
use async_trait::async_trait;
use serde_json::Value;
pub type Questions = aidash_application::ports::CompactionQuestions;

#[async_trait]
pub trait JevAsker: Send + Sync {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value>;
}

/// Jev pruning needs an approved `compactor`, and later a Decider (#108). A Run
/// without one is never pruned: removing the node's environment key also
/// removed the implicit Jev pruning of ordinary Runs (ADR 0041).
pub struct Unapproved;
#[async_trait]
impl JevAsker for Unapproved {
	async fn ask(&self, _: &Value, _: &Questions) -> Result<Value> {
		Err(aidash_application::Error::Context(
			aidash_domain::context::recovery::Failure::PruneUnavailable,
		)
		.into())
	}
}

pub struct JevClient(aidash_integrations::compaction::JevClient);
impl JevClient {
	/// One Run's transport for an approved compactor, using the Tenant's current
	/// OpenRouter Provider Credential for each Decision Attempt. The caller
	/// validates `config` against the node's Provider Credential Store.
	pub fn approved(
		config: crate::registry::CompactorConfig,
		access: std::sync::Arc<dyn aidash_application::provider_access::ProviderAccess>,
		context: aidash_application::provider_access::Context,
	) -> Result<Self> {
		let client = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.connect_timeout(std::time::Duration::from_secs(5))
			.timeout(std::time::Duration::from_secs(30))
			.build()?;
		let transport =
			aidash_integrations::compaction::JevClient::new(client, config.model, access, context)?;
		Ok(Self(transport.with_bounds(
			config.max_request_bytes,
			config.max_questions,
			config.max_response_bytes,
		)))
	}
}
#[async_trait]
impl aidash_application::ports::generation::compaction::ApprovedCompactionTransport for JevClient {
	fn check_request(
		&self,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<usize> {
		self.0.check_request(state, questions)
	}
	async fn ask(
		&self,
		attempt: uuid::Uuid,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<Value> {
		self.0.ask(attempt, state, questions).await
	}
}

#[async_trait]
impl aidash_application::ports::generation::compaction::remote::RemoteCompactionTransport
	for JevClient
{
	async fn check_credential(&self) -> aidash_application::Result<()> {
		self.0.check_remote_credential().await
	}
	fn check_request(
		&self,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<usize> {
		self.0.check_request(state, questions)
	}
	async fn ask(
		&self,
		attempt: uuid::Uuid,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<Value> {
		self.0.ask_remote(attempt, state, questions).await
	}
}
