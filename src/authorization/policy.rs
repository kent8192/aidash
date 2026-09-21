//! Tenant-scoped RBAC/ABAC evaluation shared by each transport and execution path.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ENTITIES: usize = 512;
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyBundle {
	pub tenant: String,
	#[serde(default)]
	pub subjects: BTreeMap<String, Subject>,
	#[serde(default)]
	pub groups: BTreeMap<String, Group>,
	#[serde(default)]
	pub roles: BTreeMap<String, Role>,
	#[serde(default)]
	pub policies: Vec<Policy>,
}

#[derive(
	Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
	User,
	Agent,
	Node,
	Service,
}

fn enabled() -> bool {
	true
}
fn attributes() -> Value {
	json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
	pub kind: SubjectKind,
	#[serde(default)]
	pub groups: BTreeSet<String>,
	#[serde(default)]
	pub roles: BTreeSet<String>,
	#[serde(default = "attributes")]
	pub attributes: Value,
	#[serde(default = "enabled")]
	pub enabled: bool,
	pub delegated_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Group {
	#[serde(default)]
	pub roles: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Role {
	#[serde(default)]
	pub inherits: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
	Allow,
	Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
	pub id: String,
	pub effect: Effect,
	pub subjects: SubjectSelector,
	pub actions: BTreeSet<String>,
	pub resources: ResourceSelector,
	pub condition: Option<Condition>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubjectSelector {
	#[serde(default)]
	pub any: bool,
	#[serde(default)]
	pub ids: BTreeSet<String>,
	#[serde(default)]
	pub groups: BTreeSet<String>,
	#[serde(default)]
	pub roles: BTreeSet<String>,
	#[serde(default)]
	pub kinds: BTreeSet<SubjectKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceSelector {
	pub kinds: BTreeSet<String>,
	#[serde(default)]
	pub ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operand {
	Subject { path: String },
	Resource { path: String },
	Environment { path: String },
	Literal { value: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
	All {
		#[schema(no_recursion)]
		conditions: Vec<Condition>,
	},
	Any {
		#[schema(no_recursion)]
		conditions: Vec<Condition>,
	},
	Eq {
		left: Operand,
		right: Operand,
	},
	NotEq {
		left: Operand,
		right: Operand,
	},
	Contains {
		left: Operand,
		right: Operand,
	},
	Exists {
		value: Operand,
	},
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Resource {
	pub tenant: String,
	pub kind: String,
	pub id: String,
	#[serde(default = "attributes")]
	pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
	pub subject: String,
	pub action: String,
	pub resource: Resource,
	#[serde(default = "attributes")]
	pub environment: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Decision {
	pub allowed: bool,
	pub reason: String,
	pub matched_policies: Vec<String>,
	pub effective_roles: BTreeSet<String>,
	pub revision: i64,
}

impl Decision {
	fn denied(reason: &str) -> Self {
		Self {
			allowed: false,
			reason: reason.into(),
			matched_policies: vec![],
			effective_roles: BTreeSet::new(),
			revision: 0,
		}
	}
}

pub fn identifier(value: &str) -> Result<()> {
	if value.is_empty()
		|| value.len() > 256
		|| value.chars().any(|c| c.is_whitespace() || c.is_control())
		|| value.contains('*')
	{
		return Err(Error::Invalid(
			"identifiers require 1..256 bytes without whitespace or wildcards".into(),
		));
	}
	Ok(())
}

fn selector_names(values: &BTreeSet<String>, allow_empty: bool) -> Result<()> {
	if (!allow_empty && values.is_empty()) || values.len() > MAX_ENTITIES {
		return Err(Error::Invalid("invalid selector size".into()));
	}
	for value in values {
		if value != "*" {
			identifier(value)?;
		}
	}
	Ok(())
}

fn matches_name(values: &BTreeSet<String>, name: &str) -> bool {
	values.contains("*") || values.contains(name)
}

impl PolicyBundle {
	pub fn validate(&self) -> Result<()> {
		identifier(&self.tenant)?;
		if [
			self.subjects.len(),
			self.groups.len(),
			self.roles.len(),
			self.policies.len(),
		]
		.into_iter()
		.any(|size| size > MAX_ENTITIES)
		{
			return Err(Error::Invalid(
				"policy bundles support at most 512 entries of each type".into(),
			));
		}
		for name in self.roles.keys() {
			identifier(name)?;
			self.expand_role(name, &mut BTreeSet::new(), &mut BTreeSet::new())?;
		}
		for (name, group) in &self.groups {
			identifier(name)?;
			self.require_roles(&group.roles)?;
		}
		for (name, subject) in &self.subjects {
			identifier(name)?;
			if !subject.attributes.is_object() {
				return Err(Error::Invalid(
					"subject attributes must be an object".into(),
				));
			}
			self.require_roles(&subject.roles)?;
			for group in &subject.groups {
				if !self.groups.contains_key(group) {
					return Err(Error::Invalid(format!("unknown group: {group}")));
				}
			}
			let mut chain = BTreeSet::new();
			let mut current = Some(name.as_str());
			while let Some(id) = current {
				if chain.len() >= MAX_DEPTH || !chain.insert(id) {
					return Err(Error::Invalid("delegation cycle or excessive depth".into()));
				}
				let entry = self
					.subjects
					.get(id)
					.ok_or_else(|| Error::Invalid(format!("unknown delegator: {id}")))?;
				current = entry.delegated_by.as_deref();
			}
		}
		let mut ids = BTreeSet::new();
		for policy in &self.policies {
			identifier(&policy.id)?;
			if !ids.insert(&policy.id) {
				return Err(Error::Invalid("duplicate policy id".into()));
			}
			let selectors = &policy.subjects;
			let count = selectors.ids.len()
				+ selectors.groups.len()
				+ selectors.roles.len()
				+ selectors.kinds.len();
			if (selectors.any && count != 0) || (!selectors.any && count == 0) {
				return Err(Error::Invalid(
					"select any subject explicitly, or supply subject selectors".into(),
				));
			}
			self.require_roles(&selectors.roles)?;
			for id in &selectors.ids {
				if !self.subjects.contains_key(id) {
					return Err(Error::Invalid(format!("unknown subject selector: {id}")));
				}
			}
			for group in &selectors.groups {
				if !self.groups.contains_key(group) {
					return Err(Error::Invalid(format!("unknown group selector: {group}")));
				}
			}
			selector_names(&policy.actions, false)?;
			selector_names(&policy.resources.kinds, false)?;
			selector_names(&policy.resources.ids, true)?;
			if let Some(condition) = &policy.condition {
				condition.validate(0, &mut 0)?;
			}
		}
		Ok(())
	}

	fn require_roles(&self, roles: &BTreeSet<String>) -> Result<()> {
		for name in roles {
			if !self.roles.contains_key(name) {
				return Err(Error::Invalid(format!("unknown role: {name}")));
			}
		}
		Ok(())
	}

	fn expand_role(
		&self,
		name: &str,
		path: &mut BTreeSet<String>,
		result: &mut BTreeSet<String>,
	) -> Result<()> {
		if path.contains(name) || path.len() >= MAX_DEPTH {
			return Err(Error::Invalid("role cycle or excessive depth".into()));
		}
		if result.contains(name) {
			return Ok(());
		}
		let role = self
			.roles
			.get(name)
			.ok_or_else(|| Error::Invalid(format!("unknown role: {name}")))?;
		path.insert(name.into());
		for parent in &role.inherits {
			self.expand_role(parent, path, result)?;
		}
		path.remove(name);
		result.insert(name.into());
		Ok(())
	}

	fn effective_roles(&self, subject: &Subject) -> Result<BTreeSet<String>> {
		let mut result = BTreeSet::new();
		for name in &subject.roles {
			self.expand_role(name, &mut BTreeSet::new(), &mut result)?;
		}
		for group in &subject.groups {
			let group = self
				.groups
				.get(group)
				.ok_or_else(|| Error::Invalid("unknown group".into()))?;
			for name in &group.roles {
				self.expand_role(name, &mut BTreeSet::new(), &mut result)?;
			}
		}
		Ok(result)
	}

	pub fn evaluate(&self, input: &Evaluation) -> Decision {
		if self.validate().is_err() {
			return Decision::denied("invalid_policy");
		}
		if input.validate().is_err() {
			return Decision::denied("invalid_request");
		}
		if input.resource.tenant != self.tenant {
			return Decision::denied("tenant_mismatch");
		}
		let mut decision = self.evaluate_subject(&input.subject, input);
		if !decision.allowed {
			return decision;
		}
		let mut next = self
			.subjects
			.get(&input.subject)
			.and_then(|s| s.delegated_by.as_deref());
		while let Some(id) = next {
			let ancestor = self.evaluate_subject(id, input);
			if !ancestor.allowed {
				decision.allowed = false;
				decision.reason = "delegation_denied".into();
				decision.matched_policies.extend(ancestor.matched_policies);
				decision.matched_policies.sort();
				decision.matched_policies.dedup();
				return decision;
			}
			next = self
				.subjects
				.get(id)
				.and_then(|s| s.delegated_by.as_deref());
		}
		decision
	}

	fn evaluate_subject(&self, id: &str, input: &Evaluation) -> Decision {
		let Some(subject) = self.subjects.get(id) else {
			return Decision::denied("unknown_subject");
		};
		if !subject.enabled {
			return Decision::denied("subject_disabled");
		}
		let Ok(roles) = self.effective_roles(subject) else {
			return Decision::denied("invalid_policy");
		};
		let mut decision = Decision::denied("no_matching_allow");
		decision.effective_roles = roles.clone();
		let mut deny = false;
		for policy in &self.policies {
			let selectors = &policy.subjects;
			let subject_matches = selectors.any
				|| selectors.ids.contains(id)
				|| selectors.kinds.contains(&subject.kind)
				|| !selectors.groups.is_disjoint(&subject.groups)
				|| !selectors.roles.is_disjoint(&roles);
			if !subject_matches
				|| !matches_name(&policy.actions, &input.action)
				|| !matches_name(&policy.resources.kinds, &input.resource.kind)
				|| (!policy.resources.ids.is_empty()
					&& !matches_name(&policy.resources.ids, &input.resource.id))
				|| policy
					.condition
					.as_ref()
					.is_some_and(|c| !c.matches(subject, input))
			{
				continue;
			}
			decision.matched_policies.push(policy.id.clone());
			match policy.effect {
				Effect::Allow => decision.allowed = true,
				Effect::Deny => deny = true,
			}
		}
		decision.matched_policies.sort();
		if deny {
			decision.allowed = false;
			decision.reason = "explicit_deny".into();
		} else if decision.allowed {
			decision.reason = "allowed".into();
		}
		decision
	}
}

impl Evaluation {
	pub fn validate(&self) -> Result<()> {
		for id in [
			&self.subject,
			&self.action,
			&self.resource.tenant,
			&self.resource.kind,
			&self.resource.id,
		] {
			identifier(id)?;
		}
		if !self.resource.attributes.is_object() || !self.environment.is_object() {
			return Err(Error::Invalid(
				"resource and environment attributes must be objects".into(),
			));
		}
		Ok(())
	}
}

impl Operand {
	fn validate(&self) -> Result<()> {
		let path = match self {
			Self::Subject { path } | Self::Resource { path } | Self::Environment { path } => path,
			Self::Literal { .. } => return Ok(()),
		};
		if path.len() > 1024 || !path.starts_with('/') {
			return Err(Error::Invalid(
				"attribute paths must be JSON pointers beginning with /".into(),
			));
		}
		let mut chars = path.chars();
		while let Some(c) = chars.next() {
			if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
				return Err(Error::Invalid("invalid JSON pointer escape".into()));
			}
		}
		Ok(())
	}

	fn resolve<'a>(&'a self, subject: &'a Subject, input: &'a Evaluation) -> Option<&'a Value> {
		match self {
			Self::Subject { path } => subject.attributes.pointer(path),
			Self::Resource { path } => input.resource.attributes.pointer(path),
			Self::Environment { path } => input.environment.pointer(path),
			Self::Literal { value } => Some(value),
		}
	}
}

impl Condition {
	fn validate(&self, depth: usize, count: &mut usize) -> Result<()> {
		*count += 1;
		if depth > 16 || *count > 256 {
			return Err(Error::Invalid(
				"condition exceeds expression depth or size limit".into(),
			));
		}
		match self {
			Self::All { conditions } | Self::Any { conditions } => {
				if conditions.is_empty() {
					return Err(Error::Invalid("condition groups must not be empty".into()));
				}
				for condition in conditions {
					condition.validate(depth + 1, count)?;
				}
			}
			Self::Eq { left, right }
			| Self::NotEq { left, right }
			| Self::Contains { left, right } => {
				left.validate()?;
				right.validate()?;
			}
			Self::Exists { value } => value.validate()?,
		}
		Ok(())
	}

	fn matches(&self, subject: &Subject, input: &Evaluation) -> bool {
		match self {
			Self::All { conditions } => conditions.iter().all(|c| c.matches(subject, input)),
			Self::Any { conditions } => conditions.iter().any(|c| c.matches(subject, input)),
			Self::Exists { value } => value.resolve(subject, input).is_some(),
			Self::Eq { left, right }
			| Self::NotEq { left, right }
			| Self::Contains { left, right } => {
				let (Some(left), Some(right)) =
					(left.resolve(subject, input), right.resolve(subject, input))
				else {
					return false;
				};
				match self {
					Self::Eq { .. } => left == right,
					Self::NotEq { .. } => left != right,
					Self::Contains { .. } => {
						left.as_array().is_some_and(|values| values.contains(right))
					}
					_ => unreachable!(),
				}
			}
		}
	}
}
