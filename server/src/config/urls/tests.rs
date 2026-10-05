use rstest::rstest;
use std::collections::{BTreeMap, BTreeSet};

#[rstest]
fn native_routes_match_the_committed_http_contract() {
	// Use the checked-in transport catalog rather than a generated, ignored
	// OpenAPI artifact so this contract also runs in a fresh checkout.
	let contract: serde_json::Value = serde_json::from_str(include_str!(concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/tests/bruno/contracts.json"
	)))
	.unwrap();
	let mut registered = BTreeMap::<(String, String), usize>::new();
	let router = super::routes();
	for (path, _name, _namespace, methods) in router.server_ref().get_all_routes() {
		for method in methods {
			*registered
				.entry((method.to_string(), path.clone()))
				.or_default() += 1;
		}
	}

	// Every transport operation, including SSE, Federation and static HEAD,
	// must resolve exactly once, without undocumented additional routes.
	let mut expected = BTreeMap::new();
	for endpoint in contract.as_array().unwrap() {
		let key = (
			endpoint["method"].as_str().unwrap().to_owned(),
			endpoint["path"].as_str().unwrap().to_owned(),
		);
		assert!(
			expected.insert(key, 1).is_none(),
			"duplicate catalog endpoint"
		);
	}
	assert_eq!(expected.len(), 269);
	assert_eq!(registered, expected);
}

#[rstest]
fn federation_registration_retains_the_previous_undocumented_peer_protocol() {
	// The baseline includes internal protocol routes omitted from public OpenAPI.
	let expected = BTreeSet::from([
		("GET", "/federation/v0.1/discover/{id}/{version}"),
		("GET", "/federation/v0.1/observe"),
		("GET", "/federation/v0.1/transactions/{id}/authority"),
		("GET", "/federation/v0.1/transactions/{id}/decision"),
		("POST", "/federation/v0.1/control"),
		("POST", "/federation/v0.1/discover"),
		("POST", "/federation/v0.1/offers"),
		("POST", "/federation/v0.1/scoped/dependencies/verify"),
		("POST", "/federation/v0.1/scoped/discover"),
		("POST", "/federation/v0.1/scoped/execution/admissions"),
		(
			"POST",
			"/federation/v0.1/scoped/execution/admissions/{id}/activate",
		),
		(
			"POST",
			"/federation/v0.1/scoped/execution/admissions/{id}/control",
		),
		(
			"POST",
			"/federation/v0.1/scoped/execution/admissions/{id}/messages",
		),
		(
			"POST",
			"/federation/v0.1/scoped/execution/admissions/{id}/verify",
		),
		("POST", "/federation/v0.1/scoped/execution/commands"),
		(
			"POST",
			"/federation/v0.1/scoped/execution/grants/activation",
		),
		("POST", "/federation/v0.1/scoped/execution/grants/describe"),
		("POST", "/federation/v0.1/scoped/execution/grants/snapshot"),
		("POST", "/federation/v0.1/scoped/execution/grants/verify"),
		("POST", "/federation/v0.1/scoped/execution/inspect"),
		("POST", "/federation/v0.1/scoped/execution/status"),
		("POST", "/federation/v0.1/scoped/files/chunk"),
		("POST", "/federation/v0.1/scoped/files/commit"),
		("POST", "/federation/v0.1/scoped/files/describe"),
		("POST", "/federation/v0.1/scoped/files/negotiate"),
		("POST", "/federation/v0.1/scoped/files/prepare"),
		("POST", "/federation/v0.1/scoped/files/recipients"),
		("POST", "/federation/v0.1/scoped/files/status"),
		("POST", "/federation/v0.1/scoped/generation/cancel"),
		("POST", "/federation/v0.1/scoped/generation/describe"),
		("POST", "/federation/v0.1/scoped/generation/prepare"),
		("POST", "/federation/v0.1/scoped/graph"),
		("POST", "/federation/v0.1/scoped/registry/verify"),
		("POST", "/federation/v0.1/scoped/semantic/query"),
		("POST", "/federation/v0.1/scoped/semantic/verify-operation"),
		("POST", "/federation/v0.1/scoped/usage/finalize"),
		("POST", "/federation/v0.1/scoped/usage/reserve"),
		("POST", "/federation/v0.1/scoped/usage/verify"),
		("POST", "/federation/v0.1/transactions/access"),
		("POST", "/federation/v0.1/transactions/finish"),
		("POST", "/federation/v0.1/transactions/preflight"),
		("POST", "/federation/v0.1/transactions/prepare"),
		("POST", "/federation/v0.1/transactions/reserve"),
		("POST", "/federation/v0.1/workspace"),
	]);
	// Read the real composed Reinhardt router without mutating its global reverser.
	let router = super::routes();
	let actual: BTreeSet<(String, String)> = router
		.server_ref()
		.get_all_routes()
		.into_iter()
		.flat_map(|(path, _name, _namespace, methods)| {
			methods
				.into_iter()
				.map(move |method| (method.to_string(), path.clone()))
		})
		.collect();
	let missing: Vec<_> = expected
		.into_iter()
		.filter(|(method, path)| !actual.contains(&(method.to_string(), path.to_string())))
		.collect();
	assert_eq!(missing, Vec::<(&str, &str)>::new());
}
