use super::super::tests::OperatorFixture;
use super::*;
use crate::marketplace::installations::tests::validation;
use aidash_domain::identity::Principal;

async fn selection() -> (OperatorFixture, ApprovalSet) {
	let (mut scope, id) = OperatorFixture::installed().await;
	let revision = scope.inner.revisions[&format!("{id}:1")].clone();
	scope.approvals.clear();
	scope.events.clear();
	scope.inner.calls.clear();
	let input = ApprovalSet {
		tenant: revision.tenant,
		installations: vec![PendingSelection {
			installation: id,
			revision: 1,
			digest: revision.digest,
			expected_activation_revision: 0,
		}],
		approvals: vec![ApprovalSelection {
			reference: reference(&revision.entry),
			expected_catalog_revision: 0,
		}],
	};
	(scope, input)
}
#[tokio::test]
async fn exact_pending_selection_uses_existing_catalog_administrator_authority() {
	let (mut scope, input) = selection().await;
	let result = approve_and_activate(&mut scope, &validation(), &input, "aidash://node")
		.await
		.unwrap();
	assert_eq!(result.len(), 1);
	assert_eq!(result[0].active_revision, Some(1));
	assert_eq!(result[0].activation_revision, 1);
	assert_eq!(scope.approvals.len(), 1);
	assert_eq!(scope.events.len(), 1);
	assert_eq!(scope.events[0].0, "marketplace.approval_set_activated");
	assert_eq!(
		scope.events[0].1["selection_digest"],
		digest(&serde_json::to_value(&input).unwrap())
	);
}
#[tokio::test]
async fn stale_selections_and_extra_approvals_fail_before_approval_or_activation_writes() {
	for change in 0..5 {
		let (mut scope, mut input) = selection().await;
		match change {
			0 => input.installations[0].digest = "changed".into(),
			1 => input.installations[0].expected_activation_revision = 1,
			2 => input.approvals[0].expected_catalog_revision = 1,
			3 => input.approvals.push(ApprovalSelection {
				reference: EntityRef {
					id: "unreviewed".into(),
					version: "1.0.0".into(),
				},
				expected_catalog_revision: 0,
			}),
			_ => input.installations.push(input.installations[0].clone()),
		}
		assert!(
			approve_and_activate(&mut scope, &validation(), &input, "aidash://node")
				.await
				.is_err()
		);
		assert_eq!(scope.approvals.len(), 0);
		assert_eq!(scope.events.len(), 0);
		assert_eq!(
			scope.inner.installations[&input.installations[0].installation].active_revision,
			None
		);
	}
}
#[tokio::test]
async fn missing_dependency_and_ordinary_subject_do_not_receive_implicit_approval() {
	let (mut scope, input) = selection().await;
	let id = &input.installations[0].installation;
	scope
		.inner
		.revisions
		.get_mut(&format!("{id}:1"))
		.unwrap()
		.dependencies
		.push(EntityRef {
			id: "dependency".into(),
			version: "1.0.0".into(),
		});
	assert!(matches!(
		approve_and_activate(&mut scope, &validation(), &input, "aidash://node").await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.approvals.len(), 0);
	let (mut scope, input) = selection().await;
	scope.principal = Principal::Subject {
		tenant: input.tenant.clone(),
		subject: "administrator-display-name".into(),
	};
	assert!(matches!(
		approve_and_activate(&mut scope, &validation(), &input, "aidash://node").await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.inner.calls.len(), 0);
}
#[tokio::test]
async fn replay_cannot_reactivate_a_selection_with_an_old_pointer_revision() {
	let (mut scope, input) = selection().await;
	approve_and_activate(&mut scope, &validation(), &input, "aidash://node")
		.await
		.unwrap();
	let before = scope.approvals.len();
	assert!(matches!(
		approve_and_activate(&mut scope, &validation(), &input, "aidash://node").await,
		Err(Error::Conflict(_))
	));
	assert_eq!(scope.approvals.len(), before);
}
