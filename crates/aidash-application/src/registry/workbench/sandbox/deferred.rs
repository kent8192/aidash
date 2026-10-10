//! `deferred@1` Workbench sessions carry an Exposure set across turns. Each
//! request is selected as a Run selects it, and the pure exposure tools are
//! evaluated against the pinned snapshot instead of fixtures.
use crate::{Error, Result, tools::required};
use aidash_domain::{
	exposure::{self, Capability, CapabilityIdentity, DeferredBudgets, ExposureState},
	provider::{ToolCall, ToolSpec},
	registry::{
		AgentConfig,
		bindings::{BindingSnapshot, EXPOSURE_TOOLS, ResolvedBinding},
	},
	tool::providers::ToolDescriptor,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The call outcome of a result the session computed itself.
pub const EVALUATED: &str = "evaluated";

/// Discoverable capabilities of a deferred draft. Attachments are
/// Discoverable and load their pinned body; mounted roots have no live area.
pub struct Deferred {
	config: AgentConfig,
	budgets: DeferredBudgets,
	catalog: Vec<Capability>,
	specifications: BTreeMap<String, ToolSpec>,
}

impl Deferred {
	/// `None` under `legacy@1`.
	pub fn new(snapshot: &BindingSnapshot) -> Result<Option<Self>> {
		let config = AgentConfig::from_snapshot(snapshot)?;
		let Some(budgets) = config.exposure_policy().budgets().copied() else {
			return Ok(None);
		};
		let specifications =
			crate::registry::bindings::execution::bound_specifications(snapshot, |binding, _| {
				Ok(crate::tools::plugin_specification(
					&binding.definition,
					binding.alias.as_deref().unwrap_or_default(),
				))
			})?;
		let attachments = config
			.skill_attachments
			.iter()
			.map(crate::capabilities::skills::attachment_skill)
			.collect::<Result<Vec<_>>>()?;
		let catalog = exposure::catalog(snapshot, &specifications, &attachments)?;
		Ok(Some(Self {
			config,
			budgets,
			catalog,
			specifications,
		}))
	}

	/// The resident Skill blocks and capability index of the request for
	/// `state`, and the tools it advertises.
	pub fn request(
		&self,
		snapshot: &BindingSnapshot,
		state: &ExposureState,
	) -> Result<(String, Vec<ToolSpec>)> {
		let selection = exposure::select(&self.budgets, &self.catalog, state)?;
		let mut text = String::new();
		for alias in &selection.skills {
			let capability = self
				.catalog
				.iter()
				.find(|capability| &capability.alias == alias)
				.ok_or_else(|| {
					Error::Invalid(format!("selected Skill {alias} is not in the catalog"))
				})?;
			let body = match &capability.identity {
				CapabilityIdentity::Registry(_) => {
					exposure::registry_skill_body(snapshot, capability)?
				}
				CapabilityIdentity::DirectSkill { skill_id, .. } => self
					.config
					.skill_attachments
					.iter()
					.find(|attachment| attachment.skill_id == *skill_id)
					.map(|attachment| attachment.instructions.clone())
					.ok_or_else(|| {
						Error::Invalid(format!("selected Skill {alias} is not attached"))
					})?,
			};
			text.push_str(&exposure::resident_block(capability, &body));
		}
		text.push_str(&selection.index);
		let tools = self
			.specifications
			.iter()
			.filter(|(alias, _)| selection.tools.contains(*alias))
			.map(|(_, specification)| specification.clone())
			.collect();
		Ok((text, tools))
	}

	/// The result of a call this session answers itself: a capability its
	/// request did not advertise, or a pure exposure tool. Every call of a
	/// response is decided against its own request's Exposure set; Load/Unload
	/// results are staged into `state` for the next request. `None` leaves the
	/// call to its fixture or real dispatch.
	///
	/// `fits(result, reserve)` tells whether the next test request still fits
	/// its limits with `result` recorded and `reserve` more request bytes. A
	/// session has no context compaction, so search pages and descriptions are
	/// fitted before they are retained, as the Harness fits them for a Run.
	/// When not even their smallest form fits, nothing is retained:
	/// [`Evaluated::NoRoom`].
	pub fn evaluate(
		&self,
		binding: &ResolvedBinding,
		call: &ToolCall,
		state: &mut ExposureState,
		step: i32,
		fits: &dyn Fn(&Value, usize) -> bool,
	) -> Result<Option<Evaluated>> {
		let exposed = |state: &ExposureState| -> Result<bool> {
			Ok(exposure::select(&self.budgets, &self.catalog, state)?
				.tools
				.contains(&call.name))
		};
		if !exposed(state)? {
			let message = if exposed(&state.effective())? {
				format!(
					"capability {} was loaded in this response; call it after the next model request",
					call.name
				)
			} else {
				format!(
					"capability {} is not loaded; use capability_load",
					call.name
				)
			};
			return Ok(Some(Evaluated::Result(json!({ "error": message }))));
		}
		let Some(operation) = (binding.definition.kind == "tool")
			.then(|| serde_json::from_value::<ToolDescriptor>(binding.definition.config.clone()))
			.transpose()?
			.map(|descriptor| descriptor.operation)
			.filter(|operation| EXPOSURE_TOOLS.contains(&operation.as_str()))
		else {
			return Ok(None);
		};
		let effective = state.effective();
		let input = &call.arguments;
		let output = match operation.as_str() {
			"capability_search" => exposure::search(
				&self.catalog,
				&effective,
				input["query"].as_str().unwrap_or_default(),
				input["cursor"].as_str(),
			)
			.map_err(Error::from),
			"capability_describe" => required(input, "alias")
				.and_then(|alias| Ok(exposure::describe(&self.catalog, &self.budgets, alias)?)),
			"capability_load" => required(input, "alias").and_then(|alias| {
				Ok(exposure::load(
					&self.budgets,
					&self.catalog,
					&effective,
					alias,
					required(input, "digest")?,
					step,
				)?)
			}),
			_ => required(input, "alias")
				.and_then(|alias| Ok(exposure::unload(&self.catalog, &effective, alias)?)),
		};
		let output = match output {
			Ok(output) if operation == "capability_search" => {
				let offset = crate::execution::capability_search_offset(call);
				match crate::execution::fit_capability_search_page(&output, offset, |page| {
					fits(page, 0)
				}) {
					Some(page) => page,
					None => return Ok(Some(Evaluated::NoRoom)),
				}
			}
			Ok(output) if operation == "capability_describe" => {
				match fit_description(output, fits) {
					Some(description) => description,
					None => return Ok(Some(Evaluated::NoRoom)),
				}
			}
			Ok(output) => output,
			Err(
				Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message)),
			) => {
				json!({ "error": message })
			}
			Err(error) => return Err(error),
		};
		if let Some(update) = output.get("exposure_update") {
			state.stage(serde_json::from_value(update.clone())?);
		}
		Ok(Some(Evaluated::Result(output)))
	}
}

/// A call this session answered itself.
#[derive(Debug, PartialEq)]
pub enum Evaluated {
	/// The result to record with the outcome [`EVALUATED`].
	Result(Value),
	/// Not even the smallest result leaves the next test request within its
	/// limits. Retaining one would only fail that request, so the session stops.
	NoRoom,
}

/// A description is retained only when the next request also carries the
/// definition a Load would add, since the session never compacts it away.
/// Otherwise its detail is omitted; the digest still loads the capability.
/// `None` when even that does not fit.
fn fit_description(output: Value, fits: &dyn Fn(&Value, usize) -> bool) -> Option<Value> {
	let reserve = output["bytes"].as_u64().unwrap_or(0) as usize;
	if fits(&output, reserve) {
		return Some(output);
	}
	let mut omitted = output;
	if let Some(object) = omitted.as_object_mut() {
		object.remove("detail");
	}
	omitted["deferred"] = json!(true);
	omitted["message"] = json!(
		"This description does not fit the remaining test context together with its definition; its detail was omitted. Load the capability by digest to use it."
	);
	fits(&omitted, 0).then_some(omitted)
}

/// The Exposure set a continued session ended with: the evaluated Load/Unload
/// results of its conversation, in order. A completed session activated them
/// all before its final request.
pub fn replayed(conversation: &[Value]) -> Result<ExposureState> {
	let mut state = ExposureState::default();
	for content in conversation
		.iter()
		.filter(|part| part["role"] == "tool")
		.map(|part| &part["content"])
		.filter(|content| content["outcome"] == EVALUATED)
	{
		if let Some(update) = content["result"].get("exposure_update") {
			state.apply(&serde_json::from_value(update.clone())?);
		}
	}
	Ok(state)
}

#[cfg(test)]
mod tests;
