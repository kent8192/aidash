use super::*;
use rstest::rstest;

fn http(url: &str, headers: &[(&str, &str)]) -> McpServer {
	McpServer::Remote(RemoteServer {
		url: url.parse().unwrap(),
		headers: headers
			.iter()
			.map(|(name, value)| Header {
				name: (*name).into(),
				value: (*value).into(),
			})
			.collect(),
	})
}

#[rstest]
#[case::no_url(json!({"type": "streamable-http"}))]
#[case::empty_url(json!({"type": "streamable-http", "url": ""}))]
#[case::url_not_a_string(json!({"type": "streamable-http", "url": 7}))]
#[case::relative_url(json!({"type": "streamable-http", "url": "/mcp"}))]
#[case::no_scheme(json!({"type": "streamable-http", "url": "example.com/mcp"}))]
#[case::ftp(json!({"type": "streamable-http", "url": "ftp://example.com/mcp"}))]
#[case::websocket(json!({"type": "streamable-http", "url": "wss://example.com/mcp"}))]
#[case::username(json!({"type": "streamable-http", "url": "https://user@example.com/mcp"}))]
#[case::password(json!({"type": "streamable-http", "url": "https://user:secret@example.com/mcp"}))]
#[case::empty_password(json!({"type": "streamable-http", "url": "https://:@example.com/mcp"}))]
#[case::fragment(json!({"type": "streamable-http", "url": "https://example.com/mcp#frag"}))]
#[case::empty_fragment(json!({"type": "streamable-http", "url": "https://example.com/mcp#"}))]
#[case::inner_space(json!({"type": "streamable-http", "url": "https://example.com/a b"}))]
#[case::leading_space(json!({"type": "streamable-http", "url": " https://example.com/mcp"}))]
#[case::newline(json!({"type": "streamable-http", "url": "https://example.com/mcp\n"}))]
#[case::headers_not_an_object(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": []}))]
#[case::header_value_not_a_string(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": 1}}))]
#[case::header_name_with_space(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X A": "1"}}))]
#[case::header_name_with_colon(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X:A": "1"}}))]
#[case::empty_header_name(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"": "1"}}))]
#[case::header_value_with_newline(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": "a\nb"}}))]
#[case::header_value_with_nul(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": "a\u{0}b"}}))]
#[case::header_value_with_delete(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": "a\u{7f}"}}))]
#[case::duplicate_header_in_two_casings(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-Key": "1", "x-key": "2"}}))]
#[case::reserved_host(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Host": "evil.example"}}))]
#[case::reserved_content_length(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"content-length": "0"}}))]
#[case::reserved_content_type(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Content-Type": "text/plain"}}))]
#[case::reserved_transfer_encoding(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Transfer-Encoding": "chunked"}}))]
#[case::reserved_connection(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"CONNECTION": "close"}}))]
#[case::reserved_upgrade(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Upgrade": "websocket"}}))]
#[case::reserved_te(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"TE": "trailers"}}))]
#[case::reserved_trailer(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Trailer": "X-A"}}))]
#[case::reserved_keep_alive(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Keep-Alive": "timeout=5"}}))]
#[case::reserved_proxy_connection(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Proxy-Connection": "keep-alive"}}))]
#[case::reserved_accept(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Accept": "*/*"}}))]
#[case::reserved_session_id(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"Mcp-Session-Id": "1"}}))]
#[case::reserved_protocol_version(json!({"type": "sse", "url": "https://example.com/mcp", "headers": {"MCP-Protocol-Version": "2025-06-18"}}))]
#[case::cross_variant_command(json!({"type": "streamable-http", "url": "https://example.com/mcp", "command": "node"}))]
#[case::cross_variant_env(json!({"type": "streamable-http", "url": "https://example.com/mcp", "env": {}}))]
#[case::cross_variant_cwd(json!({"type": "streamable-http", "url": "https://example.com/mcp", "cwd": "./"}))]
fn a_malformed_remote_entry_is_invalid(#[case] entry: Value) {
	assert_invalid(entry);
}

#[rstest]
#[case::authorization("Authorization")]
#[case::lowercase_authorization("authorization")]
#[case::proxy_authorization("Proxy-Authorization")]
#[case::upper_proxy_authorization("PROXY-AUTHORIZATION")]
fn credentials_are_never_carried_in_a_literal_header(#[case] name: &str) {
	let mut headers = serde_json::Map::new();
	headers.insert(name.into(), json!("Bearer secret"));
	let finding = the_only_finding(json!({
		"type": "streamable-http",
		"url": "https://example.com/mcp",
		"headers": headers,
	}));
	assert_eq!(finding.reason, Reason::ServerInvalid);
	assert_eq!(
		finding.detail,
		format!("header {name} must not carry credentials; connect the server instead")
	);
}
#[rstest]
#[case::https("https://example.com/mcp", "https://example.com/mcp")]
#[case::http_hostname("http://ollama.lan:11434/mcp", "http://ollama.lan:11434/mcp")]
#[case::http_public_hostname("http://example.com/mcp", "http://example.com/mcp")]
#[case::http_private_address("http://10.0.0.1/mcp", "http://10.0.0.1/mcp")]
#[case::http_public_address("http://203.0.113.5/mcp", "http://203.0.113.5/mcp")]
#[case::http_unspecified_address("http://0.0.0.0/mcp", "http://0.0.0.0/mcp")]
#[case::localhost_lookalike("http://localhost.example.com/mcp", "http://localhost.example.com/mcp")]
#[case::loopback_lookalike("http://127.0.0.1.example.com/mcp", "http://127.0.0.1.example.com/mcp")]
#[case::https_port_and_query(
	"https://example.com:8443/mcp?v=1",
	"https://example.com:8443/mcp?v=1"
)]
#[case::localhost("http://localhost:8080/mcp", "http://localhost:8080/mcp")]
#[case::loopback_v4("http://127.0.0.1/mcp", "http://127.0.0.1/mcp")]
#[case::loopback_range("http://127.9.9.9:9000/", "http://127.9.9.9:9000/")]
#[case::loopback_v6("http://[::1]:3000/mcp", "http://[::1]:3000/mcp")]
#[case::loopback_mapped_v6("http://[::ffff:127.0.0.1]/mcp", "http://[::ffff:7f00:1]/mcp")]
#[case::https_to_loopback("https://localhost/mcp", "https://localhost/mcp")]
#[case::host_is_normalized("HTTPS://Example.COM", "https://example.com/")]
fn remote_http_endpoints_are_retained_for_deployment_admission(
	#[case] url: &str,
	#[case] stored: &str,
) {
	assert_eq!(
		the_only_server(json!({"type": "streamable-http", "url": url})),
		http(stored, &[])
	);
}

#[test]
fn headers_keep_their_casing_and_are_listed_by_name() {
	let server = the_only_server(json!({
		"type": "streamable-http",
		"url": "https://example.com/mcp",
		"headers": {"X-Tenant": "acme", "Accept-Language": "en", "x-flags": "a\tb"},
	}));
	assert_eq!(
		server,
		http(
			"https://example.com/mcp",
			&[
				("Accept-Language", "en"),
				("X-Tenant", "acme"),
				("x-flags", "a\tb")
			]
		)
	);
}

#[rstest]
#[case("https://example.com/sse")]
#[case("http://example.com/sse")]
fn a_valid_sse_entry_is_skipped_as_an_unsupported_transport(#[case] url: &str) {
	assert_eq!(
		the_only_finding(json!({"type": "sse", "url": url, "headers": {"X-A": "1"}})),
		skipped(
			Component::Server("srv".into()),
			Reason::UnsupportedTransport,
			"sse"
		)
	);
}

#[rstest]
#[case::no_url(json!({"type": "sse"}))]
#[case::command(json!({"type": "sse", "url": "https://example.com/sse", "command": "node"}))]
fn an_invalid_sse_entry_is_invalid_rather_than_unsupported(#[case] entry: Value) {
	assert_invalid(entry);
}
