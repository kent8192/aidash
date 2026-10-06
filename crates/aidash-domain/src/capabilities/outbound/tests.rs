use super::*;
use chrono::TimeZone;
use rstest::{fixture, rstest};

#[rstest]
#[case::zero("0.9.1.1", false)]
#[case::loopback("127.0.0.1", false)]
#[case::private_a("10.2.3.4", false)]
#[case::private_b("172.16.0.1", false)]
#[case::public_b_boundary("172.15.0.1", true)]
#[case::private_c("192.168.1.1", false)]
#[case::link_local("169.254.1.1", false)]
#[case::carrier_start("100.64.0.1", false)]
#[case::carrier_end("100.127.0.1", false)]
#[case::carrier_after("100.128.0.1", true)]
#[case::protocol("192.0.0.1", false)]
#[case::documentation_a("192.0.2.1", false)]
#[case::benchmark_a("198.18.0.1", false)]
#[case::benchmark_b("198.19.0.1", false)]
#[case::documentation_b("198.51.100.1", false)]
#[case::documentation_c("203.0.113.1", false)]
#[case::multicast("224.0.0.1", false)]
#[case::broadcast("255.255.255.255", false)]
#[case::public_v4("8.8.8.8", true)]
#[case::unspecified_v6("::", false)]
#[case::loopback_v6("::1", false)]
#[case::private_v6("fc00::1", false)]
#[case::link_local_v6("fe80::1", false)]
#[case::mapped("::ffff:8.8.8.8", false)]
#[case::documentation_v6("2001:db8::1", false)]
#[case::protocol_v6("2001:100::1", false)]
#[case::protocol_boundary("2001:200::1", true)]
#[case::tunnel("2002::1", false)]
#[case::new_documentation("3fff:fff::1", false)]
#[case::new_documentation_after("3fff:1000::1", true)]
#[case::public_v6("2606:4700::1111", true)]
fn approved_dns_answers_keep_the_existing_address_classification(
	#[case] address: &str,
	#[case] allowed: bool,
) {
	assert_eq!(public_ip(address.parse().unwrap()), allowed);
}

#[rstest]
#[case::https("https://example.com/path?q=1", true)]
#[case::explicit_tls_port("https://example.com:443/path", true)]
#[case::normalized_host("https://EXAMPLE.COM/path", true)]
#[case::plaintext("http://example.com/path", false)]
#[case::credentials("https://user:sentinel@example.com/path", false)]
#[case::username("https://user@example.com/path", false)]
#[case::fragment("https://example.com/path#part", false)]
#[case::other_port("https://example.com:444/path", false)]
fn outbound_targets_require_the_same_https_origin_and_url_shape(
	#[case] target: &str,
	#[case] accepted: bool,
) {
	let origins = vec!["https://example.com".into()];
	let result = permitted_origin(target, &origins);
	if accepted {
		assert_eq!(result.unwrap().1, origins[0]);
	} else {
		assert!(
			matches!(result,Err(Error::Invalid(message)) if message=="outbound access requires an HTTPS URL without credentials on port 443")
		);
	}
}

#[rstest]
fn invalid_and_unlisted_origins_retain_distinct_failures() {
	assert!(
		matches!(permitted_origin("invalid",&[]),Err(Error::Invalid(message)) if message=="INVALID_OUTBOUND_URL")
	);
	assert!(
		matches!(permitted_origin("https://example.com/",&[]),Err(Error::Conflict(message)) if message=="OUTBOUND_OPERATOR_DENIED")
	);
	let prefix = "https://example.com/";
	let target = format!("{prefix}{}", "a".repeat(4096 - prefix.len()));
	assert!(permitted_origin(&target, &["https://example.com".into()]).is_ok());
	assert!(matches!(
		permitted_origin(&(target + "a"), &["https://example.com".into()]),
		Err(Error::Invalid(_))
	));
}

#[fixture]
fn record() -> Record {
	Record {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		owner: "owner".into(),
		area_id: Some(Uuid::from_u128(2)),
		kind: "outbound".into(),
		state: "approved".into(),
		revision: 7,
		data: json!({"url":"https://example.com/path","targets":["https://example.com"],"binding":"retained"}),
		expires_at: None,
	}
}

#[rstest]
fn durable_intent_retains_the_exact_attempt_and_deadline_fields(mut record: Record) {
	let at = Utc.timestamp_opt(1000, 0).unwrap();
	let deadline = at + chrono::Duration::seconds(90);
	attempted(&mut record, at, deadline);
	assert_eq!(record.state, "attempted");
	assert_eq!(record.data["attempted_at"], json!(at));
	assert_eq!(record.data["attempt_deadline"], json!(deadline));
	assert_eq!(record.revision, 7);
	assert_eq!(record.data["binding"], "retained");
}

#[rstest]
fn completed_network_output_preserves_the_existing_file_contract(mut record: Record) {
	let file_id = Uuid::from_u128(3);
	completed(
		&mut record,
		file_id,
		"digest".into(),
		201,
		b"hello",
		"https://example.com/final",
	);
	assert_eq!(record.state, "completed");
	assert_eq!(record.data["http_status"], 201);
	assert_eq!(record.data["final_url"], "https://example.com/final");
	assert_eq!(record.data["binding"], "retained");
	assert_eq!(
		record.data["output_file"],
		json!({"file_id":file_id,"path":format!("outbound-{}.bin",record.id),"digest":"digest","size":5,"media_type":"application/octet-stream","scope":"working","provenance":{"kind":"outbound","operation_id":record.id}})
	);
}

#[rstest]
fn uncertain_execution_discloses_prior_effects_without_a_retry_grant() {
	assert_eq!(
		failure_disclosure("OUTBOUND_EFFECT_UNCERTAIN"),
		json!({"error":{"code":"OUTBOUND_EFFECT_UNCERTAIN","message":"Outbound execution stopped; earlier effects may have occurred.","retryable":false}})
	);
}
