//! The Tenant's own policy, which a Tenant Administrator reads and replaces
//! through its Mapping (#356, #357; ADR 0021 and ADR 0026 amended).
use super::{AdministrationActor, AdministrationEvent, TenantAdministrator, rules};
use crate::apps::identity::models::{AuthorizationDecision, AuthorizationRevision};
use crate::authorization::Snapshot;
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_domain::policy::{Decision, Evaluation, PolicyBundle};
use reinhardt::db::orm::AtomicTransaction;
use serde_json::{Value, json};

impl TenantAdministrator {
	pub(super) async fn locked_for(
		&self,
		tx: &mut AtomicTransaction,
		action: &str,
		exclusive: bool,
	) -> Result<Snapshot> {
		let snapshot = self.lock(tx, exclusive).await?;
		self.require(tx, &snapshot, action, rules::COLLECTION, json!({}))
			.await?;
		Ok(snapshot)
	}

	pub(crate) async fn policy_snapshot(&self, db: &Pool) -> Result<Snapshot> {
		db.atomic(&self.scope()?, async |tx| {
			self.locked_for(tx, rules::POLICY_READ, false).await
		})
		.await
	}

	pub(crate) async fn policy_revisions(
		&self,
		db: &Pool,
		after: i64,
		limit: i64,
	) -> Result<Vec<Value>> {
		db.atomic(&self.scope()?, async |tx| {
			self.locked_for(tx, rules::POLICY_READ, false).await?;
			AuthorizationRevision::page(tx, self.tenant(), after, limit).await
		})
		.await
	}

	/// Evaluates a scenario on the current policy without writing it to the
	/// decision log, where it could not be told apart from a real decision.
	pub(crate) async fn simulate(&self, db: &Pool, input: &Evaluation) -> Result<Decision> {
		input.validate()?;
		db.atomic(&self.scope()?, async |tx| {
			let snapshot = self.locked_for(tx, rules::POLICY_READ, false).await?;
			let mut decision = snapshot.bundle.evaluate(input);
			decision.revision = snapshot.revision;
			Ok(decision)
		})
		.await
	}

	pub(crate) async fn decisions(&self, db: &Pool, after: i64, limit: i64) -> Result<Vec<Value>> {
		db.atomic(&self.scope()?, async |tx| {
			self.locked_for(tx, rules::DECISION_READ, false).await?;
			AuthorizationDecision::page(tx, self.tenant(), after, limit).await
		})
		.await
	}

	/// Replaces the whole policy. Holding `policy.update` is every authority
	/// within the Tenant, so the replacement may change this administrator's
	/// own subject; it still has to leave someone who can repair the policy.
	pub(crate) async fn replace_policy(
		&self,
		db: &Pool,
		actor: &AdministrationActor,
		expected_revision: i64,
		bundle: &PolicyBundle,
	) -> Result<Snapshot> {
		aidash_application::authorization::validate_replacement(
			self.tenant(),
			expected_revision,
			bundle,
			&self.revision_actor(),
		)?;
		db.atomic(&self.scope()?, async |tx| {
			let mut snapshot = self.locked_for(tx, rules::POLICY_UPDATE, true).await?;
			if snapshot.revision != expected_revision {
				return Err(Error::Conflict("authorization revision changed".into()));
			}
			snapshot.bundle = bundle.clone();
			self.require_remaining_administrator(tx, &snapshot.bundle, None)
				.await?;
			snapshot.revision = self.save_policy(tx, &snapshot).await?;
			actor
				.record(
					tx,
					AdministrationEvent {
						action: rules::POLICY_UPDATE,
						identity: None,
						registration: None,
						mapping: None,
						tenant: Some(self.tenant()),
						subject: None,
						invitation: None,
					},
				)
				.await?;
			Ok(snapshot)
		})
		.await
	}
}
