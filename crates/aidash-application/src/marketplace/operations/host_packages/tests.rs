use super::super::{
	approval_set::{ApprovalSelection, ApprovalSet, PendingSelection, approve_and_activate},
	tests::OperatorFixture,
};
use super::*;
use crate::ports::{Credentials, registry::CoreToolCatalog};
use aidash_domain::{
	capabilities::CoreCapabilities,
	provider::ToolSpec,
	tool::providers::{ToolDescriptor, requires_runner},
};
use std::sync::Arc;
struct CredentialsUnused;
impl Credentials for CredentialsUnused {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("operator declarations do not load secrets")
	}
}
struct Providers {
	runner: bool,
}
impl CoreToolCatalog for Providers {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		crate::registry::system::packages::HOST_GROUPS
			.iter()
			.flat_map(|(_, ops)| ops.iter())
			.map(|operation| {
				(
					operation.to_string(),
					ToolSpec {
						name: operation.to_string(),
						description: operation.to_string(),
						parameters: json!({"type":"object"}),
					},
				)
			})
			.collect()
	}
	fn provider_available(&self, descriptor: &ToolDescriptor) -> Result<()> {
		if requires_runner(&descriptor.operation) && !self.runner {
			Err(Error::Invalid(
				"PROVIDER_UNAVAILABLE: runner required".into(),
			))
		} else {
			Ok(())
		}
	}
}
fn validation(runner: bool) -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(CredentialsUnused), Arc::new(Providers { runner }))
}
fn input(groups: &[&str]) -> HostPackages {
	HostPackages {
		tenant: "owner".into(),
		groups: groups.iter().map(|s| s.to_string()).collect(),
		idempotency_key: Uuid::from_u128(901),
	}
}

#[tokio::test]
async fn host_group_staging_pins_complete_rewritten_lifecycle_without_any_approval() {
	let mut scope = OperatorFixture::new();
	let validation = validation(true);
	let result = provision(
		&mut scope,
		&validation,
		&validation,
		&input(&["shell", "python"]),
		"aidash://node",
	)
	.await
	.unwrap();
	assert_eq!(result.installations.len(), 9);
	assert!(scope.approvals.is_empty());
	assert!(
		result
			.installations
			.iter()
			.all(|r| r.installation.active_revision.is_none() && !r.approved)
	);
	let identities = result
		.installations
		.iter()
		.map(|r| reference(&r.entry))
		.collect::<Vec<_>>();
	for installation in &result.installations {
		assert!(
			installation
				.dependencies
				.iter()
				.all(|r| identities.contains(r))
		);
		if installation.entry.kind == "tool" {
			let descriptor: ToolDescriptor =
				serde_json::from_value(installation.entry.config.clone()).unwrap();
			if let Some(lifecycle) = descriptor.lifecycle {
				assert!(identities.contains(&lifecycle.poll.local()));
				assert!(identities.contains(&lifecycle.cancel.local()));
			}
		}
	}
	let approval = ApprovalSet {
		tenant: "owner".into(),
		installations: result
			.installations
			.iter()
			.map(|r| PendingSelection {
				installation: r.installation.id.clone(),
				revision: r.revision,
				digest: r.digest.clone(),
				expected_activation_revision: 0,
			})
			.collect(),
		approvals: result
			.installations
			.iter()
			.map(|r| ApprovalSelection {
				reference: reference(&r.entry),
				expected_catalog_revision: 0,
			})
			.collect(),
	};
	approve_and_activate(&mut scope, &validation, &approval, "aidash://node")
		.await
		.unwrap();
	assert_eq!(scope.approvals.len(), 9);
}
#[tokio::test]
async fn unavailable_runner_creates_no_sandbox_revision_and_retry_never_falls_back() {
	let mut scope = OperatorFixture::new();
	let validation = validation(false);
	let request = input(&["shell", "python", "task_assign"]);
	let result = provision(
		&mut scope,
		&validation,
		&validation,
		&request,
		"aidash://node",
	)
	.await
	.unwrap();
	assert_eq!(result.installations.len(), 2);
	assert_eq!(
		result.unavailable.keys().cloned().collect::<Vec<_>>(),
		vec!["python", "shell"]
	);
	let before = scope.inner.revisions.len();
	let available = validation_for_retry();
	let retried = provision(
		&mut scope,
		&available,
		&available,
		&request,
		"aidash://node",
	)
	.await
	.unwrap();
	assert_eq!(retried.installations.len(), 2);
	assert_eq!(scope.inner.revisions.len(), before);
	let mut explicit_retry = request;
	explicit_retry.idempotency_key = Uuid::from_u128(902);
	let later = provision(
		&mut scope,
		&available,
		&available,
		&explicit_retry,
		"aidash://node",
	)
	.await
	.unwrap();
	assert_eq!(later.installations.len(), 11);
	assert!(scope.approvals.is_empty());
}
fn validation_for_retry() -> DefinitionValidation {
	validation(true)
}
#[tokio::test]
async fn untrusted_operator_spelling_and_stale_source_bytes_fail_before_persistence() {
	let validation = validation(true);
	let mut scope = OperatorFixture::new();
	scope.principal = aidash_domain::identity::Principal::Subject {
		tenant: "operator".into(),
		subject: "operator".into(),
	};
	assert!(matches!(
		provision(
			&mut scope,
			&validation,
			&validation,
			&input(&["shell"]),
			"aidash://node"
		)
		.await,
		Err(Error::Forbidden)
	));
	assert!(scope.inner.calls.is_empty());
	scope.principal = aidash_domain::identity::Principal::Operator;
	let request = input(&["shell"]);
	let staged = provision(
		&mut scope,
		&validation,
		&validation,
		&request,
		"aidash://node",
	)
	.await
	.unwrap();
	let source = &staged.installations[0].installation.package_key;
	scope.inner.versions.get_mut(source).unwrap().publisher = "ordinary-subject".into();
	let mut retry = request;
	retry.idempotency_key = Uuid::from_u128(903);
	let before = scope.inner.revisions.len();
	assert!(matches!(
		provision(
			&mut scope,
			&validation,
			&validation,
			&retry,
			"aidash://node"
		)
		.await,
		Err(Error::Conflict(_))
	));
	assert_eq!(scope.inner.revisions.len(), before);
}
