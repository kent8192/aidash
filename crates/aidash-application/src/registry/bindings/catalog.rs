//! Registry admission uses immutable documents without legacy overlays.
use crate::{
	Error, Result,
	ports::{bindings::BindingCatalog, registry::DefinitionLookup},
};
use aidash_domain::registry::{
	Entry, Projection,
	bindings::{QualifiedRef, sources::NativeContext},
};
use async_trait::async_trait;

pub struct LookupCatalog<'a> {
	pub definitions: &'a mut dyn DefinitionLookup,
	pub node: &'a str,
}
#[async_trait]
impl BindingCatalog for LookupCatalog<'_> {
	async fn foreign_agent(
		&mut self,
		reference: &QualifiedRef,
	) -> Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		self.definitions.foreign_agent(self.node, reference).await
	}
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
		if reference.registry_node != self.node {
			return Err(Error::Invalid(
				"Binding origin is unavailable on this Node".into(),
			));
		}
		self.definitions
			.definition(&reference.id, &reference.version)
			.await
	}
	async fn installation(&mut self, projection: &Projection) -> Result<()> {
		self.definitions.binding_installation(projection).await
	}
	async fn source(&mut self, definition: &Entry) -> Result<()> {
		if definition.kind == "skill" {
			return Ok(());
		}
		let source: NativeContext = serde_json::from_value(definition.config.clone())?;
		source.validate(&definition.kind)?;
		Ok(())
	}
}

/// Tenant and receiver-export paths cannot borrow the operator peer credential
/// to import another Node's definitions.
pub struct LocalCatalog<'a>(pub LookupCatalog<'a>);
#[async_trait]
impl BindingCatalog for LocalCatalog<'_> {
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
		self.0.definition(reference).await
	}
	async fn installation(&mut self, projection: &Projection) -> Result<()> {
		self.0.installation(projection).await
	}
	async fn source(&mut self, definition: &Entry) -> Result<()> {
		self.0.source(definition).await
	}
}
