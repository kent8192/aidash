use super::{
	MAX_RATE_BUCKETS, PRUNE_INTERVAL, Protection, ProxyNetwork, Settings, mark_sensitive_headers,
	private_response,
};
use http::{HeaderValue, header};
use reinhardt::server::{RequestAdmission, RequestHead};
use reinhardt::{Request, Response};
use rstest::rstest;
use std::time::{Duration, Instant};

#[test]
fn rate_buckets_remain_bounded_without_scanning_on_every_admission() {
	let protection = Protection::new(Settings::default());
	for id in 0..MAX_RATE_BUCKETS {
		assert!(protection.admit(format!("auth:{id}"), 2, Duration::from_secs(60)));
	}
	assert!(!protection.admit("auth:overflow".into(), 2, Duration::from_secs(60)));
	assert!(protection.admit("auth:0".into(), 2, Duration::from_secs(60)));
	{
		let mut buckets = protection.buckets.lock().unwrap();
		assert_eq!(buckets.entries.len(), MAX_RATE_BUCKETS);
		for bucket in buckets.entries.values_mut() {
			bucket.updated = Instant::now() - Duration::from_secs(121);
		}
		buckets.admissions = PRUNE_INTERVAL - 1;
	}
	assert!(protection.admit("auth:new".into(), 2, Duration::from_secs(60)));
	assert_eq!(protection.buckets.lock().unwrap().entries.len(), 1);
}

/// A full map of expired buckets is pruned before a new key is refused,
/// even between periodic prunes.
#[test]
fn expired_buckets_never_lock_new_callers_out() {
	let protection = Protection::new(Settings::default());
	for id in 0..MAX_RATE_BUCKETS {
		assert!(protection.admit(format!("auth:{id}"), 2, Duration::from_secs(60)));
	}
	{
		let mut buckets = protection.buckets.lock().unwrap();
		for bucket in buckets.entries.values_mut() {
			bucket.updated = Instant::now() - Duration::from_secs(121);
		}
		buckets.admissions = 1;
		buckets.pruned_at = None;
	}
	assert!(protection.admit("auth:new".into(), 2, Duration::from_secs(60)));
	assert_eq!(protection.buckets.lock().unwrap().entries.len(), 1);
}

#[tokio::test]
async fn body_admission_owns_the_request_slot_and_uses_route_limits() {
	let protection = Protection::new(Settings {
		concurrency: 1,
		..Default::default()
	});
	let method = http::Method::POST;
	let headers = http::HeaderMap::new();
	let uri = "/api/workspaces/scoped/files/file/chunks".parse().unwrap();
	let admitted = RequestAdmission::admit(
		&protection,
		RequestHead {
			method: &method,
			uri: &uri,
			headers: &headers,
			remote_addr: None,
		},
	)
	.await
	.unwrap();
	assert_eq!(admitted.max_body_size, 6 * 1024 * 1024);
	let denied = RequestAdmission::admit(
		&protection,
		RequestHead {
			method: &method,
			uri: &uri,
			headers: &headers,
			remote_addr: None,
		},
	)
	.await;
	assert_eq!(
		denied.err().unwrap().status,
		http::StatusCode::SERVICE_UNAVAILABLE
	);
	drop(admitted);
	let ordinary = "/api/tasks".parse().unwrap();
	let admitted = RequestAdmission::admit(
		&protection,
		RequestHead {
			method: &method,
			uri: &ordinary,
			headers: &headers,
			remote_addr: None,
		},
	)
	.await
	.unwrap();
	assert_eq!(admitted.max_body_size, super::BODY_LIMIT as u64);
}

#[tokio::test]
async fn pre_body_overload_rejection_is_correlated_private_and_measured() {
	// Arrange: a thread-local recorder avoids modifying process-global telemetry.
	let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
	let handle = recorder.handle();
	let _recorder = metrics::set_default_local_recorder(&recorder);
	let protection = Protection::new(Settings {
		concurrency: 1,
		..Default::default()
	});
	let method = http::Method::POST;
	let mut headers = http::HeaderMap::new();
	let id = "7bb1b620-7b40-4bf5-b5b0-0a59f0c29ffb";
	headers.insert("x-request-id", HeaderValue::from_static(id));
	let uri = "/api/workspaces".parse().unwrap();
	let head = || RequestHead {
		method: &method,
		uri: &uri,
		headers: &headers,
		remote_addr: None,
	};
	let _held = RequestAdmission::admit(&protection, head()).await.unwrap();

	// Act
	let denied = RequestAdmission::admit(&protection, head())
		.await
		.err()
		.unwrap();

	// Assert
	assert_eq!(denied.status, http::StatusCode::SERVICE_UNAVAILABLE);
	assert_eq!(denied.headers["x-request-id"], id);
	assert_eq!(denied.headers[header::CACHE_CONTROL], "no-store");
	assert_eq!(denied.headers["retry-after"], "1");
	let metrics = handle.render();
	assert!(metrics.lines().any(|line| {
		line.starts_with("aidash_http_requests_total{")
			&& line.contains("route=\"/api/workspaces\"")
			&& line.contains("status=\"503\"")
			&& line.ends_with(" 1")
	}));
}

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
