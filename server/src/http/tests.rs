use super::{Protection, ProxyNetwork, Settings, mark_sensitive_headers, private_response};
use http::{HeaderValue, header};
use reinhardt::{Request, Response};
use rstest::rstest;

#[rstest]
#[case(true)]
#[case(false)]
fn every_response_cookie_is_redacted_without_changing_its_wire_value(#[case] private: bool) {
	// Arrange: the callback sets session, CSRF and login cookies independently.
	let cookies = [
		"aidash-session=session-secret; HttpOnly",
		"aidash-csrf=csrf-secret",
		"aidash-login=; Max-Age=0",
	];
	let mut response = Response::ok();
	for cookie in cookies {
		response
			.headers
			.append(header::SET_COOKIE, HeaderValue::from_static(cookie));
	}
	response
		.headers
		.insert(header::LOCATION, HeaderValue::from_static("/dashboard"));

	// Act
	let response = private_response(response, "00000000-0000-0000-0000-000000000000", private);

	// Assert: every cookie remains usable on the wire and is hidden from Debug.
	let values: Vec<_> = response
		.headers
		.get_all(header::SET_COOKIE)
		.iter()
		.collect();
	assert_eq!(
		values
			.iter()
			.map(|value| value.to_str().unwrap())
			.collect::<Vec<_>>(),
		cookies
	);
	assert!(values.iter().all(|value| value.is_sensitive()));
	let diagnostic = format!("{:?}", response.headers);
	assert!(!diagnostic.contains("session-secret"));
	assert!(!diagnostic.contains("csrf-secret"));
	assert_eq!(response.headers[header::LOCATION], "/dashboard");
	assert!(!response.headers[header::LOCATION].is_sensitive());
}

#[rstest]
fn every_request_credential_field_is_redacted() {
	let names = ["authorization", "cookie", "x-aidash-csrf"];
	let mut request = Request::builder().uri("/auth/logout").build().unwrap();
	for name in names {
		request
			.headers
			.append(name, HeaderValue::from_static("first-secret"));
		request
			.headers
			.append(name, HeaderValue::from_static("second-secret"));
	}
	request.headers.insert(
		header::CONTENT_TYPE,
		HeaderValue::from_static("application/json"),
	);

	mark_sensitive_headers(&mut request.headers, &names);

	for name in names {
		let values: Vec<_> = request.headers.get_all(name).iter().collect();
		assert_eq!(values.len(), 2);
		assert!(values.iter().all(|value| value.is_sensitive()));
	}
	let diagnostic = format!("{:?}", request.headers);
	assert!(!diagnostic.contains("first-secret"));
	assert!(!diagnostic.contains("second-secret"));
	assert!(!request.headers[header::CONTENT_TYPE].is_sensitive());
}

#[rstest]
#[case::inside_pod_range("10.4.0.0/14", "10.7.255.254", true)]
#[case::past_pod_range("10.4.0.0/14", "10.8.0.1", false)]
#[case::before_pod_range("10.4.0.0/14", "10.3.255.255", false)]
#[case::exact_ip("127.0.0.1", "127.0.0.1", true)]
#[case::other_ip("127.0.0.1", "127.0.0.2", false)]
#[case::exact_ipv6("::1", "::1", true)]
#[case::ipv6_prefix("2001:db8::/32", "2001:db8:ffff::1", true)]
#[case::ipv4_any_excludes_ipv6("0.0.0.0/0", "::1", false)]
#[case::ipv6_any_excludes_ipv4("::/0", "198.51.100.1", false)]
fn only_peers_inside_a_trusted_network_supply_the_client_address(
	#[case] network: &str,
	#[case] peer: &str,
	#[case] trusted: bool,
) {
	let protection = Protection::new(Settings {
		auth_trusted_proxy_ips: vec![network.parse().unwrap()],
		..Default::default()
	});
	let mut request = Request::builder()
		.uri("/auth/config")
		.header("x-real-ip", "198.51.100.9")
		.build()
		.unwrap();
	let peer: std::net::IpAddr = peer.parse().unwrap();
	request.remote_addr = Some(std::net::SocketAddr::new(peer, 1));

	let client = protection.auth_ip(&request).unwrap();

	let expected = if trusted {
		"198.51.100.9".parse().unwrap()
	} else {
		peer
	};
	assert_eq!(client, expected);
}

#[rstest]
#[case::host_bits("10.4.0.1/14")]
#[case::ipv4_prefix_too_long("10.0.0.0/33")]
#[case::ipv6_prefix_too_long("::/129")]
#[case::empty_prefix("10.0.0.0/")]
#[case::signed_prefix("10.0.0.0/+8")]
#[case::hostname("proxy.internal")]
fn malformed_trusted_proxy_entries_are_rejected(#[case] raw: &str) {
	assert!(raw.parse::<ProxyNetwork>().is_err());
}
