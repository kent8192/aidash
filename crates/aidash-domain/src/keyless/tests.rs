use super::*;
use rstest::rstest;

fn permitted(entries: &[&str]) -> PermittedOrigins {
	PermittedOrigins::parse(entries).expect("valid origins")
}

#[rstest]
#[case("http://127.0.0.1:8080/v1")]
#[case("http://10.0.0.7/v1")]
#[case("http://169.254.169.254/computeMetadata/v1")]
#[case("http://[::1]:11434/v1")]
#[case("http://[::ffff:127.0.0.1]/v1")]
// WHATWG parsing turns decimal and hexadecimal hosts into address literals.
#[case("http://2130706433/v1")]
#[case("http://0x7f.1/v1")]
fn non_public_address_literals_are_forbidden_unless_permitted(#[case] endpoint: &str) {
	assert_eq!(
		destination(endpoint, &PermittedOrigins::default()).unwrap(),
		Destination::Forbidden
	);
}

#[rstest]
#[case("https://embeddings.example/v1")]
#[case("http://localhost:11434/v1")]
#[case("https://93.184.216.34/v1")]
fn other_hosts_are_limited_to_public_addresses(#[case] endpoint: &str) {
	assert_eq!(
		destination(endpoint, &PermittedOrigins::default()).unwrap(),
		Destination::PublicOnly
	);
}

#[test]
fn a_permitted_origin_may_use_any_address_but_only_on_its_own_port() {
	let origins = permitted(&["http://127.0.0.1:11434", "http://ollama.lan"]);
	assert_eq!(
		destination("http://127.0.0.1:11434/v1", &origins).unwrap(),
		Destination::Permitted
	);
	assert_eq!(
		destination("http://127.0.0.1:8080/v1", &origins).unwrap(),
		Destination::Forbidden
	);
	assert_eq!(
		destination("http://ollama.lan/v1", &origins).unwrap(),
		Destination::Permitted
	);
	assert_eq!(
		destination("http://ollama.lan:8080/v1", &origins).unwrap(),
		Destination::PublicOnly
	);
	assert_eq!(
		destination("https://ollama.lan/v1", &origins).unwrap(),
		Destination::PublicOnly
	);
}

#[test]
fn permitted_origins_are_normalized_like_endpoint_origins() {
	let origins = permitted(&["http://Ollama.LAN:80", "https://models.lan:443/"]);
	assert_eq!(
		destination("http://ollama.lan/v1", &origins).unwrap(),
		Destination::Permitted
	);
	assert_eq!(
		destination("https://models.lan/v1", &origins).unwrap(),
		Destination::Permitted
	);
}

#[rstest]
#[case("ollama.lan:11434")]
#[case("ftp://ollama.lan")]
#[case("http://ollama.lan/v1")]
#[case("http://user:secret@ollama.lan")]
#[case("http://ollama.lan/?key=1")]
#[case("http://ollama.lan/#models")]
fn permitted_origins_must_be_bare_http_origins(#[case] entry: &str) {
	assert!(PermittedOrigins::parse([entry]).is_err());
}

#[test]
fn invalid_endpoints_are_rejected_before_any_decision() {
	assert!(
		destination(
			"http://user:secret@example.test/v1",
			&PermittedOrigins::default()
		)
		.is_err()
	);
	assert!(destination("file:///etc/passwd", &PermittedOrigins::default()).is_err());
}
