//! Private text has an explicit immutable source identity and separate storage.
use crate::{Result, ports::registry::DefinitionLookup};
use aidash_domain::registry::{
	Entry,
	bindings::{
		AgentBindings, Binding, BindingKind, QualifiedRef,
		sources::{NativeContext, NativeSource},
	},
	knowledge,
};
use async_trait::async_trait;
use serde_json::{Value, json};

/// Replace generated document bindings while preserving explicitly mounted Sources.
pub async fn detach(scope: &mut dyn DefinitionLookup, entry: &mut Entry, node: &str) -> Result<()> {
	let mut input: AgentBindings = serde_json::from_value(entry.config.clone())?;
	let mut retained = Vec::new();
	for binding in input.bindings {
		if binding.kind == BindingKind::Source
			&& binding.target.registry_node == node
			&& binding.target.id.starts_with("private.")
		{
			let source = scope
				.definition(&binding.target.id, &binding.target.version)
				.await?;
			if aidash_domain::registry::bindings::sources::validate_definition(&source)?
				.is_some_and(|context| {
					matches!(context.source, NativeSource::PrivateReferences { .. })
				}) {
				continue;
			}
		}
		retained.push(binding);
	}
	input.bindings = retained;
	entry.config = serde_json::to_value(input)?;
	entry.binding_normalization = None;
	Ok(())
}

pub fn attach(entry: &mut Entry, node: &str, documents: &Value) -> Result<Entry> {
	let digest = knowledge::digest(documents);
	let hash =
		aidash_domain::registry::rules::digest(&json!([node, entry.id, entry.version, digest]));
	let identity = QualifiedRef {
		registry_node: node.into(),
		id: format!("private.{}", hash.trim_start_matches("sha256:")),
		version: "1.0.0".into(),
	};
	let mut input: AgentBindings = serde_json::from_value(entry.config.clone())?;
	input.bindings.push(Binding {
		kind: BindingKind::Source,
		target: identity.clone(),
		alias: None,
		narrow: Default::default(),
		members: vec![],
	});
	input.validate()?;
	entry.config = serde_json::to_value(input)?;
	Ok(Entry {
		binding_normalization: None,
		installation: None,
		id: identity.id,
		version: identity.version,
		kind: "source".into(),
		name: [("en".into(), "Private reference documents".into())].into(),
		description: [("en".into(), "Immutable private reference context".into())].into(),
		capabilities: vec![],
		tags: vec!["private".into()],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: serde_json::to_value(NativeContext {
			schema_version: 1,
			source: NativeSource::PrivateReferences { digest },
		})?,
	})
}
pub struct Preview<'a> {
	pub scope: &'a mut dyn DefinitionLookup,
	pub source: &'a Entry,
}
#[async_trait]
impl DefinitionLookup for Preview<'_> {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		if self.source.id == id && self.source.version == version {
			return Ok(self.source.clone());
		}
		self.scope.definition(id, version).await
	}
	async fn overrides(&mut self, id: &str, version: &str) -> Result<Option<Value>> {
		self.scope.overrides(id, version).await
	}
	async fn binding_installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> Result<()> {
		self.scope.binding_installation(projection).await
	}
}
