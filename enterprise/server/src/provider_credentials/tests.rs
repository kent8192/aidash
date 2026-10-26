// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

use super::store;
use rstest::rstest;
use serde_json::{Value, json};

const KID: &str = "projects/fixture/locations/us-central1/keyRings/capability/cryptoKeys/capability/cryptoKeyVersions/1";

fn fields(value: Value) -> serde_json::Map<String, Value> {
	value.as_object().unwrap().clone()
}

fn secret_manager() -> serde_json::Map<String, Value> {
	fields(
		json!({"kind":"secret_manager","byok_project_id":"aidash-byok-fixture","environment_id":"test"}),
	)
}

fn broker(audience: &str) -> Value {
	json!({"endpoint":"https://broker.run.app/api/v1","issuer":"aidash","audience":audience,"kid":KID})
}

#[test]
fn a_secret_manager_store_is_served_by_its_credential_broker() {
	assert!(store(&secret_manager(), None).unwrap().issuer.is_none());
	assert!(
		store(&secret_manager(), Some(&broker("test")))
			.unwrap()
			.issuer
			.is_some()
	);
}

#[rstest]
#[case::other_kind(
	json!({"kind":"vault"}),
	None,
	"the Enterprise build has no vault Secret Store"
)]
#[case::foreign_field(
	json!({"kind":"secret_manager","byok_project_id":"aidash-byok-fixture","environment_id":"test","master_key":{"env":"KEY"}}),
	None,
	"invalid Secret Store: unknown field `master_key`"
)]
#[case::project(
	json!({"kind":"secret_manager","byok_project_id":"byok/project","environment_id":"test"}),
	None,
	"invalid Secret Store project or environment"
)]
#[case::environment(
	json!({"kind":"secret_manager","byok_project_id":"aidash-byok-fixture","environment_id":"Test"}),
	None,
	"invalid Secret Store project or environment"
)]
#[case::audience(
	json!({"kind":"secret_manager","byok_project_id":"aidash-byok-fixture","environment_id":"test"}),
	Some(broker("other")),
	"Credential Broker audience must match the configured Secret Store environment"
)]
#[case::broker_field(
	json!({"kind":"secret_manager","byok_project_id":"aidash-byok-fixture","environment_id":"test"}),
	Some(json!({"endpoint":"https://broker.run.app/api/v1"})),
	"invalid Credential Broker: missing field `issuer`"
)]
fn invalid_store_settings_refuse_to_start(
	#[case] config: Value,
	#[case] broker: Option<Value>,
	#[case] refusal: &str,
) {
	let error = store(&fields(config), broker.as_ref()).err().unwrap();
	assert!(error.to_string().starts_with(refusal), "{error}");
}
