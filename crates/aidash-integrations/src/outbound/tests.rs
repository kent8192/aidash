use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn an_origin_outside_the_committed_grant_is_denied_before_dns_resolution() {
	let policy = FetchPolicy {
		origins: vec!["https://not-resolvable.invalid".into()],
		output_bytes: 128,
	};
	let result = OutboundHttp
		.fetch(
			"https://not-resolvable.invalid/",
			&json!(["https://example.com"]),
			&policy,
		)
		.await;
	assert!(matches!(result, Err(Error::Forbidden)));
}

#[rstest]
#[tokio::test]
async fn a_private_dns_answer_is_denied_before_any_http_channel_is_created() {
	let policy = FetchPolicy {
		origins: vec!["https://127.0.0.1".into()],
		output_bytes: 128,
	};
	let result = OutboundHttp
		.fetch("https://127.0.0.1/", &json!(["https://127.0.0.1"]), &policy)
		.await;
	assert!(matches!(result, Err(Error::Forbidden)));
}

#[rstest]
#[tokio::test]
async fn rejected_url_credentials_do_not_enter_transport_error_text() {
	let policy = FetchPolicy {
		origins: vec!["https://example.com".into()],
		output_bytes: 128,
	};
	let error = OutboundHttp
		.fetch(
			"https://user:sentinel@example.com/",
			&json!(["https://example.com"]),
			&policy,
		)
		.await
		.unwrap_err();
	assert!(matches!(
		error,
		Error::Domain(aidash_domain::Error::Invalid(_))
	));
	assert!(!error.to_string().contains("sentinel"));
}
