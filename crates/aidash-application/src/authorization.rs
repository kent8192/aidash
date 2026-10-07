//! Policy evaluation is shared by HTTP, worker, and recovery callers.
use crate::{
	Error, Result,
	ports::{AuthorizationScope, AuthorizationStore},
};
use aidash_domain::policy::{Decision, Evaluation, PolicyBundle, identifier};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Snapshot {
	pub revision: i64,
	pub bundle: PolicyBundle,
}

pub struct Authorization {
	store: Arc<dyn AuthorizationStore>,
}

impl Authorization {
	pub fn new(store: Arc<dyn AuthorizationStore>) -> Self {
		Self { store }
	}

	pub async fn replace(
		&self,
		tenant: &str,
		expected_revision: i64,
		bundle: PolicyBundle,
		actor: &str,
	) -> Result<Snapshot> {
		validate_replacement(tenant, expected_revision, &bundle, actor)?;
		let mut transaction = self.store.begin().await?;
		let revision = transaction
			.replace(tenant, expected_revision, &bundle, actor)
			.await?;
		transaction.commit().await?;
		Ok(Snapshot { revision, bundle })
	}

	pub async fn evaluate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		let mut transaction = self.store.begin().await?;
		let decision = Self::evaluate_in(transaction.as_mut(), tenant, input).await?;
		transaction.commit().await?;
		Ok(decision)
	}

	/// Callers keep this same transaction for any protected effect and its audit.
	pub async fn evaluate_in(
		transaction: &mut dyn AuthorizationScope,
		tenant: &str,
		input: &Evaluation,
	) -> Result<Decision> {
		input.validate()?;
		identifier(tenant)?;
		let snapshot = transaction.load(tenant).await?;
		let mut decision = snapshot.bundle.evaluate(input);
		decision.revision = snapshot.revision;
		transaction
			.record_decision(tenant, input, &decision)
			.await?;
		Ok(decision)
	}

	pub async fn simulate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		input.validate()?;
		identifier(tenant)?;
		let mut transaction = self.store.begin().await?;
		let snapshot = transaction.load(tenant).await?;
		let mut decision = snapshot.bundle.evaluate(input);
		decision.revision = snapshot.revision;
		transaction.commit().await?;
		Ok(decision)
	}
}

/// Native compound effects use the same validation before opening their transaction.
pub fn validate_replacement(
	tenant: &str,
	expected_revision: i64,
	bundle: &PolicyBundle,
	actor: &str,
) -> Result<()> {
	bundle.validate()?;
	identifier(actor)?;
	if bundle.tenant != tenant || expected_revision < 0 || expected_revision == i64::MAX {
		return Err(Error::Invalid(
			"tenant mismatch or invalid expected revision".into(),
		));
	}
	Ok(())
}

pub mod catalog;

pub fn require_operator(principal: &aidash_domain::identity::Principal) -> Result<()> {
	if matches!(principal, aidash_domain::identity::Principal::Operator) {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

pub mod execution;

pub mod visibility;

pub mod visits;

pub mod lease;
pub mod workspaces;

pub mod projection;

pub mod journals;

pub mod stream;

pub mod records;

pub mod state;

pub mod worker_tasks;

pub mod tools;

pub mod runs;

pub mod run_details;

pub mod inference;

pub mod resume;

pub mod worker_entry;

pub mod commands;

pub mod peer;

pub mod home;

pub mod source;

pub mod dashboard;

pub mod desktop;
