use super::*;
use crate::marketplace::installations::{
	self,
	tests::{entry, input, published, validation},
};
use aidash_domain::marketplace::{Version, publication::initial_audience};
use aidash_domain::registry::EntityRef;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn denied_sources_consume_the_scan_budget_and_empty_pages_keep_progress() {
	// Arrange: 300 candidates are currently unavailable to this subject.
	let mut scope = published();
	scope.source_refs = (0..300)
		.map(|n| EntityRef {
			id: format!("source-{n}"),
			version: "1.0.0".into(),
		})
		.collect();
	scope
		.hidden
		.extend(scope.source_refs.iter().map(|r| r.id.clone()));
	// Act
	let first = sources(
		&mut scope,
		&SourceQuery {
			offset: 0,
			limit: 50,
		},
		"node",
	)
	.await
	.unwrap();
	let second = sources(
		&mut scope,
		&SourceQuery {
			offset: first.next_offset.unwrap(),
			limit: 50,
		},
		"node",
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(first.entries.len(), 0);
	assert_eq!(first.next_offset, Some(256));
	assert_eq!(second.entries.len(), 0);
	assert_eq!(second.next_offset, None);
	assert_eq!(
		scope.source_pages,
		vec![(0, 64), (64, 64), (128, 64), (192, 64), (256, 64)]
	);
}
#[rstest]
#[tokio::test]
async fn visible_sources_resume_after_the_last_scanned_candidate() {
	// Arrange: denied entries before and after the first authorized source.
	let mut scope = published();
	scope.entries.insert("second".into(), entry("second"));
	scope.source_refs = ["hidden", "source", "second"]
		.map(|id| EntityRef {
			id: id.into(),
			version: "1.0.0".into(),
		})
		.to_vec();
	scope.hidden.insert("hidden".into());
	// Act
	let first = sources(
		&mut scope,
		&SourceQuery {
			offset: 0,
			limit: 1,
		},
		"node",
	)
	.await
	.unwrap();
	let second = sources(
		&mut scope,
		&SourceQuery {
			offset: first.next_offset.unwrap(),
			limit: 1,
		},
		"node",
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(
		first
			.entries
			.iter()
			.map(|e| e.id.as_str())
			.collect::<Vec<_>>(),
		vec!["source"]
	);
	assert_eq!(first.next_offset, Some(2));
	assert_eq!(
		second
			.entries
			.iter()
			.map(|e| e.id.as_str())
			.collect::<Vec<_>>(),
		vec!["second"]
	);
	assert_eq!(second.next_offset, None);
}
#[rstest]
#[case("package", 0, 1, vec!["package-1"])]
#[case("package", 1, 1, vec!["package-2"])]
#[case("frozen instructions", 0, 50, vec![])]
#[tokio::test]
async fn browse_searches_only_public_summaries_after_current_authority_checks(
	#[case] q: &str,
	#[case] offset: usize,
	#[case] limit: usize,
	#[case] expected: Vec<&str>,
) {
	// Arrange: an unauthorized row sorts before two visible package versions.
	let mut scope = published();
	let version = scope.versions.remove("package").unwrap();
	for n in 0..3 {
		let key = format!("package-{n}");
		let candidate = Version {
			key: key.clone(),
			package_id: key.clone(),
			..version.clone()
		};
		scope
			.audiences
			.insert(key.clone(), initial_audience(&candidate));
		scope.versions.insert(key, candidate);
	}
	scope.hidden.insert("package-0".into());
	// Act
	let page = browse(
		&mut scope,
		&BrowseQuery {
			q: q.into(),
			offset,
			limit,
		},
		"node",
	)
	.await
	.unwrap();
	// Assert: config text is never a search field and denied rows do not consume visible offset.
	assert_eq!(
		page.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
		expected
	);
}
#[rstest]
#[tokio::test]
async fn missing_consent_reports_zero_revision_with_authority_evidence() {
	let mut scope = published();
	let result = read_consent(&mut scope, "package", "recipient")
		.await
		.unwrap();
	assert_eq!(result.revision, 0);
	assert_eq!(result.tenants.len(), 0);
	assert_eq!(
		scope.authority,
		vec![(
			format!("marketplace_consents:{}", key(&("package", "recipient"))),
			0
		)]
	);
}
#[rstest]
#[tokio::test]
async fn nonowners_cannot_read_redistribution_audience() {
	let mut scope = published();
	scope.versions.get_mut("package").unwrap().owner_tenant = "other".into();
	let result = read_consent(&mut scope, "package", "recipient").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.authority.len(), 0);
}
#[rstest]
#[case(false, 1)]
#[case(true, 0)]
#[tokio::test]
async fn installation_listing_rechecks_current_disclosure_policy(
	#[case] denied: bool,
	#[case] expected: usize,
) {
	// Arrange
	let mut scope = published();
	let request = input(&scope);
	installations::install(&mut scope, &validation(), "package", &request, "node")
		.await
		.unwrap();
	if denied {
		scope.denied = Some("installation.read".into());
	}
	// Act
	let result = list_installations(&mut scope, "node").await.unwrap();
	// Assert
	assert_eq!(result.len(), expected);
}
