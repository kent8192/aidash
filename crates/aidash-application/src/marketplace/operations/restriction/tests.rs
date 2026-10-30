use super::super::tests::DeploymentAdministratorFixture;
use super::*;
use aidash_domain::identity::{Principal, catalog::RestrictionChange};

fn widened() -> CatalogRestriction {
	CatalogRestriction {
		subjects: ["agent-a".into(), "agent-b".into()].into(),
		groups: BTreeSet::new(),
	}
}

/// Revision 1 is approved and active while revision 2 is staged.
async fn staged() -> (DeploymentAdministratorFixture, RestrictionSet) {
	let (mut scope, id) = DeploymentAdministratorFixture::installed().await;
	let mut first = scope.inner.revisions[&format!("{id}:1")].clone();
	let entry = reference(&first.entry);
	scope.approved.insert((entry.id, entry.version));
	let install = scope.inner.installations.get_mut(&id).unwrap();
	install.active_revision = Some(1);
	install.activation_revision = 1;
	install.latest_revision = 2;
	let tenant = first.tenant.clone();
	let digest = first.digest.clone();
	first.revision = 2;
	scope.inner.revisions.insert(format!("{id}:2"), first);
	scope.approvals.clear();
	scope.events.clear();
	scope.inner.calls.clear();
	let input = RestrictionSet {
		tenant,
		node: None,
		installations: vec![RestrictionSelection {
			installation: id,
			revision: 1,
			digest,
			expected_activation_revision: 1,
			expected_catalog_revision: 1,
			restriction: widened(),
		}],
	};
	(scope, input)
}

#[tokio::test]
async fn active_revision_restriction_changes_while_a_newer_revision_is_staged() {
	let (mut scope, input) = staged().await;
	let id = input.installations[0].installation.clone();
	restrict_active(&mut scope, &input).await.unwrap();
	let install = &scope.inner.installations[&id];
	assert_eq!(install.active_revision, Some(1));
	assert_eq!(install.activation_revision, 1);
	assert_eq!(install.latest_revision, 2);
	assert_eq!(scope.approvals.len(), 1);
	assert_eq!(scope.approvals[0].2, 1);
	assert!(scope.approvals[0].3);
	assert_eq!(
		scope.restrictions,
		vec![RestrictionChange::Replace(Some(widened()))]
	);
	assert!(
		!scope
			.inner
			.calls
			.iter()
			.any(|call| call == "save_installation")
	);
	assert_eq!(scope.events.len(), 1);
	assert_eq!(scope.events[0].0, "marketplace.restriction_updated");
	assert_eq!(
		scope.events[0].1["selection_digest"],
		digest(&serde_json::to_value(&input).unwrap())
	);
}

#[tokio::test]
async fn an_empty_restriction_clears_the_stored_one() {
	let (mut scope, mut input) = staged().await;
	input.installations[0].restriction = CatalogRestriction::default();
	restrict_active(&mut scope, &input).await.unwrap();
	assert_eq!(scope.restrictions, vec![RestrictionChange::Replace(None)]);
}

#[tokio::test]
async fn stale_or_unapproved_selections_fail_before_any_write() {
	for change in 0..7 {
		let (mut scope, mut input) = staged().await;
		match change {
			// The staged revision is not active, so it has no restriction to edit.
			0 => input.installations[0].revision = 2,
			1 => input.installations[0].expected_activation_revision = 2,
			2 => input.installations[0].digest = "changed".into(),
			3 => input.installations[0].expected_catalog_revision = 2,
			4 => scope.approved.clear(),
			5 => input.installations.push(input.installations[0].clone()),
			_ => input.installations.clear(),
		}
		assert!(restrict_active(&mut scope, &input).await.is_err());
		assert!(scope.approvals.is_empty());
		assert!(scope.events.is_empty());
	}
}

#[tokio::test]
async fn only_the_tenants_approving_subject_edits_a_restriction() {
	let (mut scope, input) = staged().await;
	scope.principal = Principal::Subject {
		tenant: input.tenant.clone(),
		subject: "administrator-display-name".into(),
	};
	scope.approving = None;
	assert!(matches!(
		restrict_active(&mut scope, &input).await,
		Err(Error::Forbidden)
	));
	assert!(scope.inner.calls.is_empty());
}
