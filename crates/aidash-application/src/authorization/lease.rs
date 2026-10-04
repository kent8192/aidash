//! Current authority evaluation and immutable resource attributes are shared across all entry points.
use crate::{Result, ports::authorization::lease::AuthorizationLease};
use aidash_domain::policy::{Evaluation, Resource};
use serde_json::Value;
pub fn resource(
	tenant: &str,
	context: &Value,
	kind: &str,
	id: &str,
	mut attributes: Value,
) -> Resource {
	if let (Some(attributes), Some(context)) = (attributes.as_object_mut(), context.as_object()) {
		for (key, value) in context {
			attributes
				.entry(key.clone())
				.or_insert_with(|| value.clone());
		}
	}
	Resource {
		tenant: tenant.into(),
		kind: kind.into(),
		id: id.into(),
		attributes,
	}
}
pub async fn decide(
	scope: &mut dyn AuthorizationLease,
	resource: &Resource,
	action: &str,
) -> Result<bool> {
	let mut records = vec![];
	let mut allowed = true;
	for subject in scope.subjects() {
		let input = Evaluation {
			subject: subject.clone(),
			action: action.into(),
			resource: resource.clone(),
			environment: scope.environment().clone(),
		};
		let mut decision = scope.snapshot().bundle.evaluate(&input);
		decision.revision = scope.snapshot().revision;
		allowed &= decision.allowed;
		records.push((input, decision));
	}
	scope.record(&records).await?;
	Ok(allowed)
}
#[cfg(test)]
mod tests;
