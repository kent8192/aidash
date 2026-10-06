//! Process settings and error conversion around the compaction adapter.
use crate::{Result, config};
use async_trait::async_trait;
use serde_json::Value;
pub type Questions = aidash_application::ports::CompactionQuestions;

#[async_trait]
pub trait JevAsker: Send + Sync {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value>;
}

pub struct JevClient(aidash_integrations::compaction::JevClient);
impl JevClient {
	pub fn from_env(client: reqwest::Client) -> Result<Self> {
		Self::new(
			client,
			std::env::var("AIDASH_JEV_ENDPOINT")
				.unwrap_or_else(|_| "https://api.typesafe.ai/v1/systemone".into()),
			std::env::var("AIDASH_JEV_MODEL").unwrap_or_else(|_| "jev-latest".into()),
			"AIDASH_SECRET_JEV".into(),
		)
	}
	pub fn new(
		client: reqwest::Client,
		endpoint: String,
		model: String,
		credential_env: String,
	) -> Result<Self> {
		config::validate_endpoint(&endpoint)?;
		crate::bootstrap::compaction_transport(client, endpoint, model, credential_env).map(Self)
	}
	pub fn approved(config: crate::registry::CompactorConfig) -> Result<Self> {
		config.validate()?;
		let client = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.connect_timeout(std::time::Duration::from_secs(5))
			.timeout(std::time::Duration::from_secs(30))
			.build()?;
		let transport = Self::new(client, config.endpoint, config.model, config.credential_env)?;
		Ok(Self(transport.0.with_bounds(
			config.max_request_bytes,
			config.max_questions,
			config.max_response_bytes,
		)))
	}
}
#[async_trait]
impl JevAsker for JevClient {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value> {
		aidash_application::ports::CompactionClassifier::ask(&self.0, state, questions)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl aidash_application::ports::CompactionClassifier for JevClient {
	async fn ask(&self, state: &Value, questions: &Questions) -> aidash_application::Result<Value> {
		aidash_application::ports::CompactionClassifier::ask(&self.0, state, questions).await
	}
}

impl aidash_application::ports::generation::compaction::ApprovedCompactionTransport for JevClient {
	fn check_request(
		&self,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<usize> {
		self.0.check_request(state, questions)
	}
}

use crate::registry::CompactorValidation;

#[async_trait]
impl aidash_application::ports::generation::compaction::remote::RemoteCompactionTransport
	for JevClient
{
	fn check_credential(&self) -> aidash_application::Result<()> {
		self.0.check_remote_credential()
	}
	fn check_request(
		&self,
		state: &Value,
		questions: &Questions,
	) -> aidash_application::Result<usize> {
		self.0.check_request(state, questions)
	}
	async fn ask(&self, state: &Value, questions: &Questions) -> aidash_application::Result<Value> {
		self.0.ask_remote(state, questions).await
	}
}
