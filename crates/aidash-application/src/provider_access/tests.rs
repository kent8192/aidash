use super::*;
use rstest::rstest;

fn permitting(origins: &[&str]) -> KeylessAccess {
	KeylessAccess::permitting(PermittedOrigins::parse(origins).unwrap())
}

async fn resolve(access: &KeylessAccess, endpoint: &str) -> Result<Access> {
	access
		.resolve(&Context::default(), endpoint, &Source::Keyless)
		.await
}

#[rstest]
#[case("http://127.0.0.1:11434/v1")]
#[case("http://169.254.169.254/computeMetadata/v1")]
#[case("http://[fd00::1]/v1")]
#[tokio::test]
async fn private_address_literals_are_refused_without_a_permission(#[case] endpoint: &str) {
	assert!(matches!(
		resolve(&KeylessAccess::default(), endpoint).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn a_permitted_private_origin_connects_directly() {
	let access = resolve(
		&permitting(&["http://127.0.0.1:11434"]),
		"http://127.0.0.1:11434/v1",
	)
	.await
	.unwrap();
	assert_eq!(access.endpoint, "http://127.0.0.1:11434/v1");
	assert_eq!(access.keyless, Some(Destination::Permitted));
}

#[tokio::test]
async fn host_names_are_left_to_the_public_only_resolver() {
	let access = resolve(&KeylessAccess::default(), "http://localhost:11434/v1")
		.await
		.unwrap();
	assert_eq!(access.keyless, Some(Destination::PublicOnly));
}

#[tokio::test]
async fn keyless_access_still_refuses_tenant_sources() {
	assert!(matches!(
		permitting(&["http://127.0.0.1:11434"])
			.resolve(
				&Context::default(),
				"http://127.0.0.1:11434/v1",
				&Source::Tenant {
					provider: "openrouter".into()
				},
			)
			.await,
		Err(Error::Invalid(_))
	));
}

#[tokio::test]
async fn a_fixture_permission_reaches_every_clone() {
	let access = KeylessAccess::default();
	let runtime_copy = access.clone();
	access
		.permit_origin_of("http://127.0.0.1:11434/v1")
		.unwrap();
	let resolved = resolve(&runtime_copy, "http://127.0.0.1:11434/v1")
		.await
		.unwrap();
	assert_eq!(resolved.keyless, Some(Destination::Permitted));
	assert!(matches!(
		resolve(&runtime_copy, "http://127.0.0.1:8080/v1").await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn keyless_data_protection_distinguishes_tenant_endpoints_from_deployment_origins() {
	use aidash_domain::data_protection::{DataProtection, Level};
	for level in [Level::NoTraining, Level::ZeroDataRetention] {
		let context = Context {
			data_protection: DataProtection {
				level,
				..Default::default()
			},
			..Default::default()
		};
		let rejected = KeylessAccess::default()
			.resolve(&context, "https://model.example/v1", &Source::Keyless)
			.await
			.unwrap_err();
		assert!(rejected.to_string().contains("Tenant Keyless endpoints"));
		let permitted = permitting(&["http://127.0.0.1:11434"]);
		assert!(
			permitted
				.resolve(&context, "http://127.0.0.1:11434/v1", &Source::Keyless)
				.await
				.unwrap_err()
				.to_string()
				.contains("Tenant Keyless endpoints")
		);
		let deployment = permitted
			.with_deployment_hosted(PermittedOrigins::parse(["http://127.0.0.1:11434"]).unwrap())
			.resolve(&context, "http://127.0.0.1:11434/v1", &Source::Keyless)
			.await
			.unwrap();
		assert_eq!(deployment.keyless, Some(Destination::Permitted));
	}
	assert!(
		KeylessAccess::default()
			.resolve(
				&Context::default(),
				"https://model.example/v1",
				&Source::Keyless
			)
			.await
			.is_ok()
	);
}

#[tokio::test]
async fn keyless_residency_uses_the_deployments_hosting_region() {
	use aidash_domain::data_protection::{DataProtection, Region};
	let context = Context {
		data_protection: DataProtection {
			data_region_restricted: true,
			..Default::default()
		}
		.in_deployment(Some(Region::Eu)),
		..Default::default()
	};
	for region in [None, Some(Region::Us), Some(Region::Eu)] {
		let access = KeylessAccess::default()
			.with_deployment_hosted(PermittedOrigins::parse(["https://model.example"]).unwrap())
			.with_deployment_region(region);
		let result = access
			.resolve(&context, "https://model.example/v1", &Source::Keyless)
			.await;
		assert_eq!(result.is_ok(), region == Some(Region::Eu));
		if let Err(error) = result {
			assert!(error.to_string().contains("outside the required region"));
		}
	}
}
