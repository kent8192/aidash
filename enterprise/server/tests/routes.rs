//! Each Offering of the Enterprise build mounts exactly the routes the
//! Community catalog and the Enterprise catalog list for it.
use aidash_domain::offering::Offering;
use rstest::rstest;
use std::collections::BTreeMap;

/// The method and path of every route `catalog` lists for `offering`. An entry
/// without `offerings` serves every Offering.
fn catalog(catalog: &str, offering: Offering) -> BTreeMap<(String, String), usize> {
	let catalog: serde_json::Value = serde_json::from_str(catalog).unwrap();
	catalog
		.as_array()
		.unwrap()
		.iter()
		.filter(|endpoint| {
			endpoint.get("offerings").is_none_or(|offerings| {
				offerings
					.as_array()
					.unwrap()
					.iter()
					.any(|served| served.as_str() == Some(offering.as_str()))
			})
		})
		.map(|endpoint| {
			(
				(
					endpoint["method"].as_str().unwrap().to_owned(),
					endpoint["path"].as_str().unwrap().to_owned(),
				),
				1,
			)
		})
		.collect()
}

#[rstest]
#[case(Offering::Cloud)]
#[case(Offering::Dedicated)]
fn each_offering_mounts_the_community_and_enterprise_routes_it_serves(#[case] offering: Offering) {
	aidash_server::bootstrap::build::install(&aidash_enterprise_server::BUILD);
	let community = catalog(
		include_str!("../../../server/tests/bruno/contracts.json"),
		offering,
	);
	let enterprise = catalog(include_str!("../routes.json"), offering);
	assert!(
		enterprise
			.keys()
			.all(|route| !community.contains_key(route)),
		"the Community catalog lists no Enterprise route"
	);
	let expected: BTreeMap<_, _> = community.into_iter().chain(enterprise).collect();
	let mut mounted = BTreeMap::<(String, String), usize>::new();
	let router = aidash_server::config::urls::routes_for(offering);
	for (path, _name, _namespace, methods) in router.server_ref().get_all_routes() {
		for method in methods {
			*mounted
				.entry((method.to_string(), path.clone()))
				.or_default() += 1;
		}
	}
	assert_eq!(mounted, expected);
}
