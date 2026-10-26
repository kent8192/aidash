use super::*;
use secrecy::ExposeSecret;
use serde_json::json;

fn hosts(values: &[&str]) -> Vec<String> {
	values.iter().map(|value| (*value).to_owned()).collect()
}

fn payload(hosts: serde_json::Value, v: serde_json::Value) -> Vec<u8> {
	serde_json::to_vec(&json!({"v": v, "hosts": hosts, "value": "canary-value"})).unwrap()
}

#[test]
fn round_trips_and_normalizes_its_hosts() {
	let bytes = seal(
		&hosts(&["API.Example.com", "api.example.com", "b.example"]),
		"canary-value",
	)
	.unwrap();
	assert_eq!(
		serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
		json!({
			"v": 1,
			"hosts": ["api.example.com", "b.example"],
			"value": "canary-value"
		})
	);
	let opened = open(&bytes).unwrap();
	assert_eq!(opened.hosts, ["api.example.com", "b.example"]);
	assert_eq!(opened.value.expose_secret(), "canary-value");
}

#[test]
fn refuses_every_payload_that_is_not_exactly_a_sealed_secret() {
	let too_many: Vec<String> = (0..=MAX_HOSTS)
		.map(|index| format!("h{index}.example"))
		.collect();
	for bytes in [
		payload(json!([]), json!(1)),
		payload(json!(too_many), json!(1)),
		payload(json!(["api.example.com"]), json!(0)),
		payload(json!(["api.example.com"]), json!(2)),
		payload(json!(["api.example.com"]), json!("1")),
		payload(json!(["API.example.com"]), json!(1)),
		payload(json!(["api.example.com:443"]), json!(1)),
		payload(json!(["https://api.example.com"]), json!(1)),
		payload(json!(["api.example.com/path"]), json!(1)),
		payload(json!(["b.example", "a.example"]), json!(1)),
		payload(json!(["a.example", "a.example"]), json!(1)),
	] {
		assert!(open(&bytes).is_err(), "{bytes:?}");
	}
	for bytes in [
		&b""[..],
		b"not json",
		b"{\"v\":1,\"hosts\":[\"a.example\"]}",
		b"{\"v\":1,\"hosts\":[\"a.example\"],\"value\":\"x\",\"extra\":true}",
		b"{\"v\":1,\"hosts\":[\"a.example\"],\"value\":\"\xff\"}",
	] {
		assert!(open(bytes).is_err(), "{bytes:?}");
	}
}

#[test]
fn sealing_invalid_hosts_is_refused_without_a_payload() {
	let too_many: Vec<String> = (0..=MAX_HOSTS)
		.map(|index| format!("h{index}.example"))
		.collect();
	for invalid in [
		hosts(&[]),
		too_many,
		hosts(&["https://api.example.com"]),
		hosts(&["api.example.com:443"]),
	] {
		assert!(seal(&invalid, "canary-value").is_err(), "{invalid:?}");
	}
}

#[test]
fn the_error_never_mentions_a_value() {
	assert!(!SealedError.to_string().contains("canary"));
	assert_eq!(MAX_HOSTS, 16);
}
