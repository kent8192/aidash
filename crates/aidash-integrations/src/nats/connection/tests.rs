//! Unit tests for services::bus::connection.
use super::*;

#[rstest::rstest]
fn transport_addresses_and_errors_do_not_disclose_url_credentials() {
	for url in [
		"nats://fixture:secret%40value@localhost:4222",
		"nats://fixture-token@localhost:4222",
	] {
		let (address, _) = options(url, ConnectOptions::new()).unwrap();
		assert_eq!(address, "nats://localhost:4222");
	}
	let error = options(
		"nats://fixture:secret%FF@localhost:4222",
		ConnectOptions::new(),
	)
	.unwrap_err()
	.to_string();
	assert!(!error.contains("secret"));
	assert!(!error.contains("fixture"));
}
