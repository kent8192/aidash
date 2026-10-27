use super::*;
use rstest::rstest;

#[rstest]
#[case::https("https://mcp.example.com", "https://mcp.example.com")]
#[case::empty_path("https://mcp.example.com/", "https://mcp.example.com")]
#[case::path_is_kept("https://mcp.example.com/v1/mcp", "https://mcp.example.com/v1/mcp")]
#[case::trailing_slash("https://mcp.example.com/v1/mcp/", "https://mcp.example.com/v1/mcp")]
#[case::scheme_and_host_are_lowercased("HTTPS://MCP.Example.COM/v1", "https://mcp.example.com/v1")]
#[case::default_https_port("https://mcp.example.com:443/v1", "https://mcp.example.com/v1")]
#[case::default_http_port("http://127.0.0.1:80/mcp", "http://127.0.0.1/mcp")]
#[case::other_https_port("https://mcp.example.com:8443/mcp", "https://mcp.example.com:8443/mcp")]
#[case::loopback_v4("http://127.0.0.1:3000/mcp", "http://127.0.0.1:3000/mcp")]
#[case::loopback_range("http://127.9.9.9/mcp", "http://127.9.9.9/mcp")]
#[case::loopback_v6("http://[::1]:3000/mcp", "http://[::1]:3000/mcp")]
#[case::https_loopback("https://[::1]/mcp", "https://[::1]/mcp")]
#[case::path_percent_encoding_is_kept(
	"https://mcp.example.com/a%2Fb",
	"https://mcp.example.com/a%2Fb"
)]
fn canonical_resources_keep_only_their_semantic_parts(#[case] input: &str, #[case] expected: &str) {
	assert_eq!(canonical_resource(input).unwrap(), expected);
}

#[rstest]
#[case::query("https://mcp.example.com/mcp?tenant=acme")]
#[case::empty_query("https://mcp.example.com/mcp?")]
#[case::fragment("https://mcp.example.com/mcp#section")]
#[case::empty_fragment("https://mcp.example.com/mcp#")]
#[case::plain_http_on_a_hostname("http://mcp.example.com/mcp")]
#[case::plain_http_on_localhost("http://localhost:3000/mcp")]
#[case::plain_http_on_a_private_address("http://10.0.0.1/mcp")]
#[case::other_scheme("ftp://mcp.example.com/mcp")]
#[case::relative("mcp.example.com/mcp")]
#[case::missing_host("https://")]
#[case::user_information("https://user:secret@mcp.example.com/mcp")]
#[case::empty_user_information("https://@mcp.example.com/mcp")]
#[case::whitespace("https://mcp.example.com/a b")]
#[case::control("https://mcp.example.com/mcp\n")]
fn a_resource_that_cannot_be_an_audience_is_refused(#[case] input: &str) {
	assert!(matches!(
		canonical_resource(input),
		Err(Refusal::Invalid(_))
	));
}

fn scopes(values: &[&str]) -> Vec<String> {
	values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn requested_scopes_are_sorted_and_deduplicated() {
	assert_eq!(
		admissible_scopes(
			&scopes(&["files:write", "files:read", "files:write"]),
			None,
			None,
			None
		)
		.unwrap(),
		scopes(&["files:read", "files:write"])
	);
}

#[test]
fn every_present_bound_must_admit_a_requested_scope() {
	let requested = scopes(&["files:read", "files:write"]);
	let declared = scopes(&["files:read", "files:write"]);
	let supported = scopes(&["files:read"]);
	let ceiling = scopes(&["files:write"]);
	assert!(admissible_scopes(&requested, Some(&declared), Some(&supported), None).is_err());
	assert!(admissible_scopes(&requested, None, Some(&supported), None).is_err());
	assert!(admissible_scopes(&requested, None, None, Some(&ceiling)).is_err());
	assert!(
		admissible_scopes(
			&requested,
			Some(&declared),
			Some(&declared),
			Some(&declared)
		)
		.is_ok()
	);
}

#[rstest]
#[case::space("files read")]
#[case::double_quote("files\"read")]
#[case::backslash("files\\read")]
#[case::empty("")]
#[case::non_ascii("файлы")]
fn a_scope_that_is_not_an_rfc_6749_token_is_refused(#[case] scope: &str) {
	assert!(matches!(
		admissible_scopes(&[scope.to_owned()], None, None, None),
		Err(Refusal::Invalid(_))
	));
}

#[test]
fn every_printable_token_character_is_admitted() {
	let scope = "!#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[]^_`abcdefghijklmnopqrstuvwxyz{|}~";
	assert_eq!(
		admissible_scopes(&[scope.to_owned()], None, None, None).unwrap(),
		[scope]
	);
}

#[test]
fn an_empty_request_admits_nothing() {
	assert_eq!(
		admissible_scopes(&[], None, None, None).unwrap(),
		Vec::<String>::new()
	);
}

#[test]
fn the_owner_s_client_wins_over_the_tenant_s() {
	let person = OAuthClient {
		id: Uuid::from_u128(1),
		tenant: "acme".into(),
		owner: Some("alice".into()),
		issuer: "https://auth.example.com".into(),
		client_id: "person".into(),
		secret_resource: None,
		pinned_version: None,
		created_at: Utc::now(),
		updated_at: Utc::now(),
		revision: 1,
	};
	let tenant = OAuthClient {
		id: Uuid::from_u128(2),
		owner: None,
		client_id: "tenant".into(),
		..person.clone()
	};
	assert_eq!(
		pick_client(Some(&person), Some(&tenant)).map(|client| client.client_id.as_str()),
		Some("person")
	);
	assert_eq!(
		pick_client(None, Some(&tenant)).map(|client| client.client_id.as_str()),
		Some("tenant")
	);
	assert!(pick_client(None, None).is_none());
}

#[test]
fn a_client_is_bound_to_its_issuer_host() {
	let client = OAuthClient {
		id: Uuid::from_u128(1),
		tenant: "acme".into(),
		owner: None,
		issuer: "https://auth.example.com:8443/tenant".into(),
		client_id: "client".into(),
		secret_resource: None,
		pinned_version: None,
		created_at: Utc::now(),
		updated_at: Utc::now(),
		revision: 1,
	};
	assert_eq!(client.host(), "auth.example.com");
	assert_eq!(
		OAuthClient {
			issuer: "https://[::1]:8443".into(),
			..client
		}
		.host(),
		"[::1]"
	);
}
