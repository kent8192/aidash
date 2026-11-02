//! Bounded, policy-filtered Run and Human Request reads within one transaction.
use crate::Result;
use aidash_domain::{HumanRequest, RawRun, RunInspection, policy::Resource};
use async_trait::async_trait;
use uuid::Uuid;

#[derive(Clone, Copy, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HumanState {
	#[default]
	Pending,
	Answered,
	All,
}

#[async_trait]
pub trait Scope: Send {
	async fn require_task(&mut self, id: Uuid) -> Result<()>;
	async fn require_run(&mut self, id: Uuid) -> Result<RunInspection>;
	async fn visible_run(&mut self, id: Uuid) -> Result<Option<RunInspection>>;
	async fn runs(&mut self, task: Uuid, cursor: Option<Uuid>) -> Result<Vec<RawRun>>;
	async fn humans(
		&mut self,
		run: Option<Uuid>,
		state: HumanState,
		cursor: Option<Uuid>,
	) -> Result<Vec<HumanRequest>>;
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
}
