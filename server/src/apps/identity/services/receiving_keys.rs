//! Identity owns the complete receiving-trust lifecycle transaction and locks.
use super::super::{
	models::AuthorizationBundle,
	repositories::{nodes, receiving_keys},
};
use crate::{Result, database::native::Pool};
use aidash_application::{
	ports::federation::inbound::{ReceivingKeyOwner, ReceivingKeys},
	tenancy::TenancyScope,
};
use aidash_domain::{
	identity::tenant_administration::NODE_USE,
	nodes::NodeName,
	policy::{Evaluation, Resource},
	tenancy::TenantName,
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use serde_json::json;

/// The protocol owner validates its real immutable journal using this exact
/// transaction and locked registration owner. Never pass a cached admission bool.
#[async_trait]
pub(crate) trait RecoveryProof: Send + Sync {
	async fn validate(
		&self,
		tx: &mut dyn TransactionExecutor,
		owner: &ReceivingKeyOwner,
	) -> Result<bool>;
}

pub(crate) async fn subject_may_use(
	pool: &Pool,
	tenant: &TenantName,
	receiver: &NodeName,
	subject: &str,
) -> Result<bool> {
	pool.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
		let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant.as_str(), false).await?;
		if !aidash_domain::identity::authority::enabled(&snapshot.bundle, subject) {
			return Ok(false);
		}
		crate::authorization::identity::require_live_subject(tx, &snapshot, subject).await?;
		let node = nodes::get_live(tx, tenant.as_str(), receiver.as_str()).await?;
		let evaluation = Evaluation {
			subject: subject.to_owned(),
			action: NODE_USE.into(),
			resource: Resource {
				tenant: tenant.to_string(),
				kind: "node".into(),
				id: receiver.to_string(),
				attributes: json!({"label": node.label, "default": node.default}),
			},
			environment: json!({"transport": "federation"}),
		};
		Ok(snapshot.bundle.evaluate(&evaluation).allowed)
	})
	.await
}

pub(crate) async fn get(pool: &Pool, owner: &ReceivingKeyOwner) -> Result<Option<ReceivingKeys>> {
	pool.atomic(
		&TenancyScope::Tenant(receiving_keys::tenant(owner).clone()),
		async |tx| receiving_keys::get(tx, owner).await,
	)
	.await
}

pub(crate) async fn advance(
	pool: &Pool,
	owner: &ReceivingKeyOwner,
	expected_keys_revision: i64,
	expected_authorization_revision: i64,
	keys: &aidash_domain::nodes::keys::TrustedDeploymentKeys,
) -> Result<bool> {
	let tenant = receiving_keys::tenant(owner);
	pool.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
		// Deletion takes exclusive policy then capacity locks in this order.
		let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant.as_str(), false).await;
		match snapshot {
			Err(crate::Error::TenantDeleted | crate::Error::NotFound(_)) => {
				return Ok(false);
			}
			other => {
				other?;
			}
		}
		let ReceivingKeyOwner::Registration { node, .. } = owner;
		nodes::lock_tenant(tx, tenant.as_str()).await?;
		match nodes::get_live(tx, tenant.as_str(), node.as_str()).await {
			Err(crate::Error::NotFound(_)) => return Ok(false),
			other => {
				other?;
			}
		}
		receiving_keys::advance(
			tx,
			owner,
			expected_keys_revision,
			expected_authorization_revision,
			keys,
		)
		.await
	})
	.await
}

pub(crate) async fn advance_recovery(
	pool: &Pool,
	owner: &ReceivingKeyOwner,
	expected_keys_revision: i64,
	expected_authorization_revision: i64,
	keys: &aidash_domain::nodes::keys::TrustedDeploymentKeys,
	proof: &dyn RecoveryProof,
) -> Result<bool> {
	let ReceivingKeyOwner::Registration { tenant, .. } = owner;
	pool.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
		match AuthorizationBundle::lock_snapshot(tx, tenant.as_str(), false).await {
			Err(crate::Error::TenantDeleted | crate::Error::NotFound(_)) => {
				return Ok(false);
			}
			other => {
				other?;
			}
		}
		nodes::lock_tenant(tx, tenant.as_str()).await?;
		// No Node catalog/deletion read: permanent name ownership and the real
		// immutable journal bind this retained registration, even after purge.
		if receiving_keys::get_locked(tx, owner).await?.is_none() {
			return Ok(false);
		}
		if !proof.validate(tx, owner).await? {
			return Ok(false);
		}
		receiving_keys::advance_recovery(
			tx,
			owner,
			expected_keys_revision,
			expected_authorization_revision,
			keys,
		)
		.await
	})
	.await
}
