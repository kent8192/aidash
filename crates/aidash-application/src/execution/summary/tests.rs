use super::*;
use aidash_domain::{
	registry::{EntityRef, Entry},
	semantic::remote::{Binding, Provider},
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Mutex;

fn model() -> EntityRef {
	EntityRef {
		id: "summarizer".into(),
		version: "1.0.0".into(),
	}
}
fn entry(kind: &str) -> Entry {
	serde_json::from_value(json!({"id":"summarizer","version":"1.0.0","kind":kind,"name":{"en":"Summarizer"},"description":{"en":""},"config":{"model":"fixture-summarizer"}})).unwrap()
}
fn summarizer() -> SummaryProvider {
	SummaryProvider {
		model: model(),
		definition_digest: digest(&serde_json::to_value(entry("model")).unwrap()),
	}
}
fn pin() -> Provider {
	Provider {
		node_id: "aidash://executor".into(),
		entry: model(),
		digest: summarizer().definition_digest,
		configuration_digest: format!("sha256:{}", "c".repeat(64)),
	}
}
fn required_home(summarizer: Option<Provider>) -> Binding {
	Binding::RequiredHome {
		home_lineage: vec![],
		execution_lineage: vec![],
		version: 1,
		index_revision: 0,
		index_digest: format!("sha256:{}", "a".repeat(64)),
		embedding: Box::new(Provider {
			node_id: "aidash://home".into(),
			entry: EntityRef {
				id: "embedding".into(),
				version: "1.0.0".into(),
			},
			digest: format!("sha256:{}", "b".repeat(64)),
			configuration_digest: format!("sha256:{}", "b".repeat(64)),
		}),
		native: None,
		compactor: None,
		summarizer: summarizer.map(Box::new),
	}
}

struct Port {
	remote: bool,
	binding: Binding,
	approved: bool,
	kind: &'static str,
	calls: Mutex<Vec<String>>,
}
impl Port {
	fn local() -> Self {
		Self {
			remote: false,
			binding: Binding::Disabled {},
			approved: true,
			kind: "model",
			calls: Mutex::new(vec![]),
		}
	}
	fn remote(binding: Binding) -> Self {
		Self {
			remote: true,
			binding,
			..Self::local()
		}
	}
	fn calls(&self) -> Vec<String> {
		self.calls.lock().unwrap().clone()
	}
}
#[async_trait]
impl SummaryAuthorization for Port {
	fn remote(&self) -> bool {
		self.remote
	}
	fn node_id(&self) -> &str {
		"aidash://executor"
	}
	async fn refresh(&self) -> Result<()> {
		self.calls.lock().unwrap().push("refresh".into());
		Ok(())
	}
	async fn remote_binding(&self) -> Result<Binding> {
		self.calls.lock().unwrap().push("binding".into());
		Ok(self.binding.clone())
	}
	async fn catalog_entry(&self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.calls.lock().unwrap().push(action.into());
		assert_eq!(reference, &model());
		if !self.approved {
			return Err(Error::Forbidden);
		}
		Ok(entry(self.kind))
	}
	async fn charge_generated(&self, _: &SummaryProvider, bytes: i64) -> Result<()> {
		assert_eq!(bytes, 64);
		self.calls.lock().unwrap().push("charge".into());
		Ok(())
	}
}
fn unavailable(result: Result<()>) -> bool {
	matches!(result, Err(Error::Context(Failure::SummaryUnavailable)))
}

#[tokio::test]
async fn local_approval_precedes_generated_charges() {
	let port = Port::local();
	authorize(&port, &summarizer(), 64).await.unwrap();
	assert_eq!(
		port.calls(),
		["refresh", "registry.read", "model.infer", "charge"]
	);
}

#[tokio::test]
async fn an_unapproved_summarizer_is_unavailable_and_never_charged() {
	let port = Port {
		approved: false,
		..Port::local()
	};
	assert!(unavailable(authorize(&port, &summarizer(), 64).await));
	assert!(!port.calls().contains(&"charge".into()));
}

#[tokio::test]
async fn a_changed_or_non_model_definition_is_unavailable() {
	let mut changed = summarizer();
	changed.definition_digest = format!("sha256:{}", "f".repeat(64));
	let port = Port::local();
	assert!(unavailable(authorize(&port, &changed, 64).await));
	let port = Port {
		kind: "compactor",
		..Port::local()
	};
	assert!(unavailable(authorize(&port, &summarizer(), 64).await));
	assert!(!port.calls().contains(&"charge".into()));
}

#[tokio::test]
async fn remote_required_home_without_a_summarizer_pin_is_unavailable() {
	for binding in [Binding::Disabled {}, required_home(None)] {
		let port = Port::remote(binding);
		assert!(unavailable(authorize(&port, &summarizer(), 64).await));
		assert_eq!(port.calls(), ["refresh", "binding"]);
	}
}

#[tokio::test]
async fn remote_pin_must_name_the_exact_summarizer_on_this_node() {
	let port = Port::remote(required_home(Some(pin())));
	authorize(&port, &summarizer(), 64).await.unwrap();
	assert_eq!(
		port.calls(),
		["refresh", "binding"],
		"remote Runs never use the executor catalog or local charges"
	);
	for change in ["node", "version", "digest"] {
		let mut provider = pin();
		match change {
			"node" => provider.node_id = "aidash://other".into(),
			"version" => provider.entry.version = "2.0.0".into(),
			_ => provider.digest = format!("sha256:{}", "e".repeat(64)),
		}
		let port = Port::remote(required_home(Some(provider)));
		assert!(
			unavailable(authorize(&port, &summarizer(), 64).await),
			"{change}"
		);
	}
}
