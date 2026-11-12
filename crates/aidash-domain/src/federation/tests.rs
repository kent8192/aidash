use super::*;
use serde_json::json;

fn agent(config: serde_json::Value) -> Entry {
	serde_json::from_value(json!({
		"id":"qa","version":"2.1.0","kind":"agent",
		"name":{"en":"QA"},"description":{"en":"Checks work"},
		"capabilities":["review"],"tags":["quality"],"languages":["en","ja"],"skills":["lint"],
		"config":config
	}))
	.unwrap()
}

#[test]
fn a_card_has_only_what_a_model_chooses_an_agent_by() {
	let entry = agent(json!({
		"model":{"id":"fixture-model","version":"1.0.0"},
		"instructions":"Secret instructions",
		"bindings":[{"kind":"memory"}]
	}));
	let card = serde_json::to_value(AgentCard::from(&entry)).unwrap();
	assert_eq!(
		card,
		json!({
			"id":"qa","version":"2.1.0","kind":"agent",
			"name":{"en":"QA"},"description":{"en":"Checks work"},
			"capabilities":["review"],"tags":["quality"],"languages":["en","ja"],"skills":["lint"],
			"model":"fixture-model"
		})
	);
	assert!(!card.to_string().contains("Secret instructions"));
}

#[test]
fn a_card_omits_the_model_when_the_configuration_names_none() {
	let card = serde_json::to_value(AgentCard::from(&agent(json!({})))).unwrap();
	assert!(card.get("model").is_none(), "{card}");
}

#[test]
fn discovery_cards_keep_the_node_and_error_shape() {
	let discovery = Discovery {
		agents: vec![DiscoveredAgent {
			node_id: "aidash://node-a".into(),
			entity: agent(json!({"model":{"id":"m","version":"1"}})),
		}],
		errors: vec![PeerError {
			node_id: "aidash://node-b".into(),
			error: "unavailable".into(),
		}],
	};
	let cards = serde_json::to_value(DiscoveryCards::from(&discovery)).unwrap();
	assert_eq!(cards["agents"][0]["node_id"], "aidash://node-a");
	assert_eq!(cards["agents"][0]["entity"]["id"], "qa");
	assert_eq!(cards["errors"][0]["node_id"], "aidash://node-b");
	// The whole definition still travels in `Discovery` itself.
	let whole = serde_json::to_value(&discovery).unwrap();
	assert!(whole["agents"][0]["entity"].get("config").is_some());
}
