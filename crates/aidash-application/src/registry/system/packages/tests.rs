use super::*;
use crate::ports::{Credentials, registry::CoreToolCatalog};
use aidash_domain::{
	capabilities::CoreCapabilities, provider::ToolSpec, tool::providers::requires_runner,
};
use std::sync::Arc;
struct Credential;
impl Credentials for Credential {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("declarations must not load secrets")
	}
}
struct Provider {
	runner: bool,
}
impl CoreToolCatalog for Provider {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		HOST_GROUPS
			.iter()
			.flat_map(|(_, operations)| operations.iter())
			.chain(
				[
					"file_search",
					"file_read",
					"skill_list",
					"skill_load",
					"skill_read",
				]
				.iter(),
			)
			.map(|operation| {
				(
					operation.to_string(),
					ToolSpec {
						name: operation.to_string(),
						description: operation.to_string(),
						parameters: json!({"type":"object"}),
					},
				)
			})
			.collect()
	}
	fn provider_available(&self, descriptor: &ToolDescriptor) -> Result<()> {
		if requires_runner(&descriptor.operation) && !self.runner {
			Err(Error::Invalid(
				"PROVIDER_UNAVAILABLE: admitted runner required".into(),
			))
		} else {
			Ok(())
		}
	}
}
fn validation(runner: bool) -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(Credential), Arc::new(Provider { runner }))
}
#[test]
fn six_operator_groups_have_only_provider_owned_declarations_and_complete_lifecycles() {
	let validation = validation(true);
	let groups = declarations(&Principal::Operator, &validation, "aidash://node").unwrap();
	assert_eq!(groups.len(), 6);
	for group in groups {
		assert_eq!(group.bundle.kind, "bundle");
		let bundle: BundleConfig = serde_json::from_value(group.bundle.config.clone()).unwrap();
		assert_eq!(bundle.members.len(), group.operations.len());
		for entry in &group.operations {
			let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
			assert_eq!(descriptor.tier, ToolTier::Host);
			assert!(descriptor.transport.is_none());
			if let Some(lifecycle) = descriptor.lifecycle {
				for member in [lifecycle.poll, lifecycle.cancel] {
					assert!(bundle.members.contains(&member));
				}
			}
		}
		group.verify_provider(&validation).unwrap();
	}
}
#[test]
fn missing_runner_excludes_sandbox_defaults_without_fallback_and_other_owners_cannot_create_them() {
	let validation = validation(false);
	let selected = HOST_GROUPS
		.iter()
		.map(|(name, _)| name.to_string())
		.collect::<Vec<_>>();
	let plan = configured_defaults(
		&Principal::Operator,
		&validation,
		&validation,
		"aidash://node",
		&selected,
	)
	.unwrap();
	assert_eq!(plan.pending.len(), 4);
	assert_eq!(
		plan.unavailable.keys().cloned().collect::<Vec<_>>(),
		vec!["python", "shell"]
	);
	assert!(
		plan.unavailable
			.values()
			.all(|reason| reason.contains("PROVIDER_UNAVAILABLE"))
	);
	assert!(
		declarations(
			&Principal::Subject {
				tenant: "operator".into(),
				subject: "operator".into()
			},
			&validation,
			"aidash://node"
		)
		.is_err()
	);
	assert!(
		configured_defaults(
			&Principal::Operator,
			&validation,
			&validation,
			"aidash://node",
			&["unknown".into()]
		)
		.is_err()
	);
}
