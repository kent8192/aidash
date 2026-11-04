use super::*;
use serde_json::json;

#[test]
fn an_external_service_cannot_open_a_tunnel_or_select_an_insecure_destination() {
	let valid = json!({"base_url":"https://api.example.test/v1/","methods":["GET","POST"],"secret":"api-key"});
	let service: ExternalService = serde_json::from_value(valid.clone()).unwrap();
	service.validate().unwrap();
	for (key, value) in [
		("base_url", json!("http://api.example.test/")),
		("base_url", json!("https://user:password@api.example.test/")),
		("base_url", json!("https://api.example.test/?token=value")),
		("methods", json!(["CONNECT"])),
		("methods", json!(["TRACE"])),
		("methods", json!(["GET", "GET"])),
		("authentication", json!({"type":"header","name":"Host"})),
		(
			"authentication",
			json!({"type":"header","name":"x-forwarded-host"}),
		),
		(
			"authentication",
			json!({"type":"header","name":"x-aidash-correlation"}),
		),
	] {
		let mut rejected = valid.clone();
		rejected[key] = value;
		assert!(
			serde_json::from_value::<ExternalService>(rejected)
				.unwrap()
				.validate()
				.is_err()
		);
	}
	assert!(
		Limits {
			requests: 1001,
			..Limits::default()
		}
		.lower_than(Limits::default())
		.is_err()
	);
}

#[test]
fn operation_budgets_accumulate_across_requests_and_refuse_overflow() {
	let limits = Limits {
		sent_bytes: 10,
		received_bytes: 12,
		requests: 2,
	};
	let usage = Usage::default()
		.debit(limits, Debit::Request)
		.unwrap()
		.debit(limits, Debit::Sent(6))
		.unwrap()
		.debit(limits, Debit::Received(12))
		.unwrap()
		.debit(limits, Debit::Request)
		.unwrap()
		.debit(limits, Debit::Sent(4))
		.unwrap();
	assert_eq!(
		usage,
		Usage {
			sent_bytes: 10,
			received_bytes: 12,
			requests: 2
		}
	);
	for debit in [
		Debit::Sent(1),
		Debit::Received(1),
		Debit::Request,
		Debit::Sent(u64::MAX),
	] {
		assert!(
			usage
				.debit(limits, debit)
				.unwrap_err()
				.to_string()
				.contains("egress_limit_exceeded")
		);
	}
}

#[test]
fn scratch_scope_host_allowlist_respects_dns_label_boundaries() {
	let hosts = AllowedHosts {
		hosts: vec!["registry.npmjs.org".into(), "*.example.com".into()],
		allow_all: false,
	};
	for host in ["REGISTRY.NPMJS.ORG", "a.example.com", "a.b.example.com"] {
		assert!(hosts.permits(host, false), "{host}");
	}
	for host in ["example.com", "badexample.com", "example.com.evil", ""] {
		assert!(!hosts.permits(host, false), "{host}");
	}
	let all = AllowedHosts {
		hosts: vec![],
		allow_all: true,
	};
	assert!(!all.permits("other.example", false));
	assert!(all.permits("other.example", true));
}

#[test]
fn scratch_scope_daily_budget_defaults_to_100_gib_per_direction() {
	assert_eq!(DailyBudget::default().sent_bytes, 100 * (1 << 30));
	assert_eq!(DailyBudget::default().received_bytes, 100 * (1 << 30));
}

#[test]
fn recorded_bytes_always_count_and_report_a_reached_limit() {
	let limits = Limits {
		sent_bytes: 10,
		received_bytes: 12,
		requests: 2,
	};
	let (usage, exhausted) = Usage::default().record(limits, Debit::Sent(9));
	assert!(!exhausted);
	// The batch that crosses the limit is still counted in full.
	let (usage, exhausted) = usage.record(limits, Debit::Sent(5));
	assert!(exhausted);
	assert_eq!(usage.sent_bytes, 14);
	// Received bytes are a separate counter; reaching it exactly is exhausted.
	let (usage, exhausted) = usage.record(limits, Debit::Received(12));
	assert!(exhausted);
	assert_eq!(usage.received_bytes, 12);
	// A debit that adds nothing never reports exhaustion.
	assert!(!usage.record(limits, Debit::Sent(0)).1);
	let (usage, _) = usage.record(limits, Debit::Received(u64::MAX));
	assert_eq!(usage.received_bytes, u64::MAX);
}
