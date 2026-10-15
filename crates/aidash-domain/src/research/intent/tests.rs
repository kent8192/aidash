use super::*;
use crate::{
	registry::{
		Entry,
		bindings::{QualifiedRef, ResolvedDefinition},
		rules,
	},
	tool::providers::{core_descriptor, research_descriptor},
};
use rstest::rstest;
use serde_json::json;

const NODE: &str = "aidash://node-a";

fn tool(id: &str, descriptor: ToolDescriptor) -> ResolvedDefinition {
	let definition: Entry = serde_json::from_value(json!({
		"id": id,
		"version": "1.0.0",
		"kind": "tool",
		"name": {"en": id},
		"description": {"en": id},
		"config": descriptor,
	}))
	.unwrap();
	ResolvedDefinition {
		identity: QualifiedRef {
			registry_node: NODE.into(),
			id: id.into(),
			version: "1.0.0".into(),
		},
		digest: rules::digest(&serde_json::to_value(&definition).unwrap()),
		definition,
	}
}

fn snapshot(remote: bool, definitions: Vec<ResolvedDefinition>) -> BindingSnapshot {
	BindingSnapshot {
		schema_version: 1,
		agent: QualifiedRef {
			registry_node: NODE.into(),
			id: "researcher".into(),
			version: "1.0.0".into(),
		},
		remote,
		bindings: vec![],
		definitions,
		foreign_agents: vec![],
	}
}

#[rstest]
fn readable_pages_in_any_order_are_accepted() {
	// Arrange
	let pages = vec![
		"https://example.com/notes".to_owned(),
		"http://example.org/a?b=c".to_owned(),
	];

	// Act
	let checked = validate(&pages);

	// Assert
	assert_eq!(checked, Ok(()));
}

#[rstest]
#[case("ftp://example.com/file")]
#[case("example.com/page")]
#[case("https://user:secret@example.com/")]
#[case("https://example.com/#section")]
#[case("https://example.com/\npath")]
fn addresses_that_cannot_be_read_are_refused(#[case] page: &str) {
	// Act
	let refused = validate(&[page.to_owned()]);

	// Assert
	assert_eq!(refused, Err(InvalidUrlIntent::Address(page.to_owned())));
}

#[rstest]
fn a_repeated_page_is_refused() {
	// Arrange
	let page = "https://example.com/".to_owned();

	// Act
	let refused = validate(&[page.clone(), page.clone()]);

	// Assert
	assert_eq!(refused, Err(InvalidUrlIntent::Duplicate(page)));
}

#[rstest]
fn more_than_the_maximum_is_refused() {
	// Arrange
	let pages = (0..=MAX_URL_INTENTS)
		.map(|index| format!("https://example.com/{index}"))
		.collect::<Vec<_>>();

	// Act
	let refused = validate(&pages);

	// Assert
	assert_eq!(refused, Err(InvalidUrlIntent::TooMany));
}

#[rstest]
#[case::page_reading(research_descriptor(NODE, "research.page@1", "fetch").unwrap(), true)]
#[case::fixture_page_reading(research_descriptor(NODE, "research.fixture@1", "fetch").unwrap(), true)]
#[case::search_only(research_descriptor(NODE, "research.brave@1", "search").unwrap(), false)]
#[case::find_only(research_descriptor(NODE, "research.page@1", "find").unwrap(), false)]
#[case::another_tool(core_descriptor(NODE, "outbound_get").unwrap(), false)]
fn only_a_bound_page_reading_operation_can_act_on_a_url_intent(
	#[case] descriptor: ToolDescriptor,
	#[case] reads: bool,
) {
	// Arrange
	let snapshot = snapshot(false, vec![tool("aidash.bound", descriptor)]);

	// Act
	let acts = reads_pages(&snapshot);

	// Assert
	assert_eq!(acts, reads);
}

#[rstest]
fn a_remote_run_never_acts_on_a_url_intent() {
	// Arrange
	let snapshot = snapshot(
		true,
		vec![tool(
			"aidash.research.page.fetch",
			research_descriptor(NODE, "research.page@1", "fetch").unwrap(),
		)],
	);

	// Act
	let acts = reads_pages(&snapshot);

	// Assert
	assert!(!acts);
}
