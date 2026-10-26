use super::*;
use chrono::TimeDelta;

#[test]
fn base32_lower_encodes_without_padding() {
	assert_eq!(base32_lower(b""), "");
	assert_eq!(base32_lower(b"f"), "my");
	assert_eq!(base32_lower(b"foo"), "mzxw6");
	assert_eq!(base32_lower(b"foobar"), "mzxw6ytboi");
}

#[test]
fn a_label_carries_at_least_128_bits_and_is_dns_safe() {
	let first = PreviewLabel::generate().unwrap();
	let second = PreviewLabel::generate().unwrap();
	assert_ne!(first, second);
	assert_eq!(first.as_str().len(), 28);
	assert!(
		first
			.as_str()
			.bytes()
			.all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
	);
	assert_eq!(
		first.host("aidashusercontent.app"),
		format!("{}.aidashusercontent.app", first.as_str())
	);
	PreviewLabel::parse(first.as_str()).unwrap();
}

#[test]
fn a_label_rejects_uppercase_padding_and_short_values() {
	for value in ["MZXW6", "mzxw6===", "mzxw", "", "mzxw6/a"] {
		assert!(PreviewLabel::parse(value).is_err(), "{value}");
	}
}

#[test]
fn a_secret_hashes_to_a_stable_256_bit_digest() {
	let secret = PreviewSecret::generate().unwrap();
	assert_eq!(secret.digest().len(), 64);
	assert!(secret.digest().bytes().all(|b| b.is_ascii_hexdigit()));
	assert_eq!(secret.digest(), secret.digest());
	assert_ne!(secret, PreviewSecret::generate().unwrap());
	assert_eq!(PreviewSecret::parse(secret.expose()).unwrap(), secret);
	assert!(PreviewSecret::parse("short").is_err());
}

#[test]
fn a_ticket_is_single_use_expires_and_binds_its_viewer() {
	let now = Utc::now();
	let ticket = Ticket {
		expires_at: now + TimeDelta::seconds(TICKET_SECONDS),
		used_at: None,
		subject: "alice".into(),
		origin: "https://dashboard.example".into(),
	};
	ticket
		.accept(now, "alice", "https://dashboard.example")
		.unwrap();
	assert!(
		ticket
			.accept(now, "bob", "https://dashboard.example")
			.is_err()
	);
	assert!(
		ticket
			.accept(now, "alice", "https://other.example")
			.is_err()
	);
	let used = Ticket {
		used_at: Some(now),
		..ticket.clone()
	};
	assert!(
		used.accept(now, "alice", "https://dashboard.example")
			.is_err()
	);
	let expired = Ticket {
		expires_at: now,
		..ticket
	};
	assert!(
		expired
			.accept(now, "alice", "https://dashboard.example")
			.is_err()
	);
}

#[test]
fn the_content_security_policy_confines_the_page_to_its_own_origin() {
	let policy = content_security_policy("https://dashboard.example").unwrap();
	assert!(policy.starts_with("default-src 'self';"));
	assert!(policy.contains("connect-src 'self';"));
	assert!(policy.contains("script-src 'self' 'unsafe-inline';"));
	assert!(policy.contains("frame-ancestors https://dashboard.example;"));
	assert!(!policy.contains("http://"));
	assert!(
		content_security_policy("https://dashboard.example/path").is_err(),
		"a frame ancestor must be an origin"
	);
	assert!(content_security_policy("data:text/html,x").is_err());
}

#[test]
fn the_frame_ancestor_is_canonicalized_and_the_port_is_kept() {
	assert_eq!(
		content_security_policy("HTTPS://Dashboard.Example:8443/").unwrap(),
		content_security_policy("https://dashboard.example:8443").unwrap()
	);
}

#[test]
fn a_preview_domain_must_be_a_registrable_domain_or_localhost() {
	assert_eq!(
		preview_domain("aidashusercontent.app").unwrap(),
		"aidashusercontent.app"
	);
	assert_eq!(preview_domain("LOCALHOST").unwrap(), "localhost");
	assert!(preview_domain("preview.example.com").is_err());
	assert!(preview_domain("co.uk").is_err());
	assert!(preview_domain("127.0.0.1").is_err());
	assert!(preview_domain("https://example.com").is_err());
	assert!(preview_domain("example.com/path").is_err());
	assert!(preview_domain("_preview.example.com").is_err());
}

#[test]
fn the_preview_domain_must_not_share_a_registrable_domain_with_the_dashboard() {
	refuse_shared_registrable_domain("aidashusercontent.app", "https://app.example.com").unwrap();
	refuse_shared_registrable_domain("localhost", "http://localhost:8080").unwrap();
	refuse_shared_registrable_domain("aidashusercontent.app", "http://127.0.0.1:8000").unwrap();
	assert!(
		refuse_shared_registrable_domain("preview.example.com", "https://app.example.com").is_err()
	);
	assert!(
		refuse_shared_registrable_domain("example.co.uk", "https://app.example.co.uk").is_err()
	);
	assert!(refuse_shared_registrable_domain("localhost", "https://app.example.com").is_err());
}

#[test]
fn a_label_is_suffixed_to_the_configured_domain_only() {
	assert_eq!(label_of("abc.example.com:8443", "example.com"), Some("abc"));
	assert_eq!(label_of("abc.example.com", "example.com"), Some("abc"));
	assert_eq!(label_of("abc.evil-example.com", "example.com"), None);
	assert_eq!(label_of("example.com", "example.com"), None);
	assert_eq!(label_of("a.b.example.com", "example.com"), None);
	assert_eq!(label_of("abc.example.com:bad", "example.com"), None);
	assert_eq!(request_host("[::1]:8960"), Some("::1"));
}
