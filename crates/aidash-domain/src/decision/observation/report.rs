//! Bounded calibration aggregates. Category maxima are risk scores, not human-denial probabilities.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportRange {
	pub start: DateTime<Utc>,
	pub end: DateTime<Utc>,
}
impl ReportRange {
	pub fn validate(&self) -> Result<()> {
		if self.end <= self.start
			|| self.end.signed_duration_since(self.start).num_seconds() > 31 * 86_400
		{
			return Err(Error::Invalid(
				"observation report range must be positive and at most 31 days".into(),
			));
		}
		Ok(())
	}
	pub fn contains(&self, at: DateTime<Utc>) -> bool {
		at >= self.start && at < self.end
	}
}
/// Every denominator travels with its rate; zero denominators serialize as null (N/A).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rate {
	pub numerator: u64,
	pub denominator: u64,
	pub value: Option<f64>,
}
impl Rate {
	pub fn new(numerator: u64, denominator: u64) -> Self {
		Self {
			numerator,
			denominator,
			value: (denominator != 0).then(|| numerator as f64 / denominator as f64),
		}
	}
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportGroup {
	pub decider: QualifiedRef,
	pub configuration_digest: String,
	pub definition_digest: String,
	pub settings_revision: i64,
	pub kind: ApprovalKind,
	pub operation_class: Option<String>,
	pub effect_class: Option<String>,
	pub route: Option<u32>,
	pub provider_contract: Option<String>,
	pub model: Option<String>,
	pub hosts: Vec<String>,
	pub effective_threshold: Probability,
}
impl ReportGroup {
	pub fn from_frame(
		frame: &ObservationFrame,
		evidence: Option<&ObservationEvidence>,
	) -> Result<Self> {
		frame.validate()?;
		if let Some(evidence) = evidence {
			evidence.validate(frame)?;
		}
		let pin = frame
			.settings
			.decider
			.as_ref()
			.ok_or_else(|| Error::Invalid("observer is disabled".into()))?;
		let route = evidence.and_then(|e| e.routes.last());
		Ok(Self {
			decider: pin.identity.clone(),
			configuration_digest: pin.configuration_digest.clone(),
			definition_digest: pin.definition_digest.clone(),
			settings_revision: frame.settings.revision,
			kind: frame.kind,
			operation_class: frame.operation_class.clone(),
			effect_class: frame.effect_class.clone(),
			route: route.map(|r| r.route),
			provider_contract: route.map(|r| r.provider_contract.clone()),
			model: route.map(|r| r.model.clone()),
			hosts: route.map_or_else(Vec::new, |r| r.hosts.clone()),
			effective_threshold: route.map_or(frame.settings.restrictions.threshold, |r| {
				r.effective_threshold
			}),
		})
	}
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfusionMatrix {
	pub shadow_allow_human_allow: u64,
	pub shadow_allow_human_deny: u64,
	pub shadow_block_human_allow: u64,
	pub shadow_block_human_deny: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exclusions {
	pub rule: u64,
	pub auto: u64,
	pub bypass: u64,
	pub expired: u64,
	pub cancelled: u64,
	pub revoked: u64,
	pub pending: u64,
	pub missing_observations: u64,
	pub unavailable_observations: u64,
	pub pending_observations: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RiskBand {
	pub lower_inclusive: f64,
	pub upper: f64,
	pub upper_inclusive: bool,
	pub observations: u64,
	pub explicit_labels: u64,
	pub human_denials: u64,
	pub empirical_human_denial_rate: Rate,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportMetrics {
	pub total_approvals: u64,
	pub available_observations: u64,
	pub explicit_labels: u64,
	pub comparable: u64,
	pub confusion: ConfusionMatrix,
	pub exclusions: Exclusions,
	pub agreement: Rate,
	pub false_allow_among_human_denials: Rate,
	pub false_allow_among_shadow_allows: Rate,
	pub false_block_among_human_approvals: Rate,
	pub availability: Rate,
	pub label_coverage: Rate,
	pub comparable_coverage: Rate,
	pub risk_bands: Vec<RiskBand>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupReport {
	pub group: ReportGroup,
	pub metrics: ReportMetrics,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationReport {
	pub range: ReportRange,
	pub metrics: ReportMetrics,
	pub groups: Vec<GroupReport>,
}
/// Private per-approval identities support deduplication but are never serialized into aggregates.
#[derive(Debug, Clone)]
pub struct ReportSample {
	pub approval: Uuid,
	pub created_at: DateTime<Utc>,
	pub group: Option<ReportGroup>,
	pub outcome: Option<ObservationOutcome>,
	pub settlement: SettlementClass,
	pub maximum_category_probability: Option<Probability>,
}
impl ReportSample {
	pub fn observed(
		frame: &ObservationFrame,
		evidence: Option<&ObservationEvidence>,
		settlement: SettlementClass,
	) -> Result<Self> {
		let group = ReportGroup::from_frame(frame, evidence)?;
		let maximum_category_probability = evidence
			.filter(|e| matches!(e.outcome, ObservationOutcome::Available { .. }))
			.and_then(|e| e.routes.last())
			.and_then(|r| {
				r.answers
					.values()
					.max_by(|a, b| a.value().total_cmp(&b.value()))
					.copied()
			});
		Ok(Self {
			approval: frame.approval,
			created_at: frame.created_at,
			group: Some(group),
			outcome: Some(evidence.map_or(ObservationOutcome::Pending, |e| e.outcome.clone())),
			settlement,
			maximum_category_probability,
		})
	}
}
/// Callers supply all approvals (including disabled/missing observations), under tenant authorization.
/// Duplicate rows cannot silently inflate denominators, and the interval is [start,end).
pub fn report(range: ReportRange, samples: &[ReportSample]) -> Result<ObservationReport> {
	range.validate()?;
	let mut seen = std::collections::BTreeSet::new();
	let mut all = Vec::new();
	let mut grouped: BTreeMap<String, (ReportGroup, Vec<&ReportSample>)> = BTreeMap::new();
	for sample in samples.iter().filter(|s| range.contains(s.created_at)) {
		if sample.approval.is_nil() || !seen.insert(sample.approval) {
			return Err(Error::Invalid(
				"duplicate or invalid approval in report".into(),
			));
		}
		let available = matches!(sample.outcome, Some(ObservationOutcome::Available { .. }));
		if available != sample.maximum_category_probability.is_some()
			|| matches!(
				sample.outcome,
				Some(ObservationOutcome::Available {
					verdict: Verdict::Blocked(BlockReason::Unavailable)
				})
			) {
			return Err(Error::Invalid(
				"report availability lacks category probabilities".into(),
			));
		}
		all.push(sample);
		if let Some(group) = &sample.group {
			let key = serde_json::to_string(group)?;
			grouped
				.entry(key)
				.or_insert_with(|| (group.clone(), vec![]))
				.1
				.push(sample);
		}
	}
	Ok(ObservationReport {
		range,
		metrics: metrics(&all),
		groups: grouped
			.into_values()
			.map(|(group, samples)| GroupReport {
				group,
				metrics: metrics(&samples),
			})
			.collect(),
	})
}
fn metrics(samples: &[&ReportSample]) -> ReportMetrics {
	let mut confusion = ConfusionMatrix::default();
	let mut exclusions = Exclusions::default();
	let mut available = 0;
	let mut labels = 0;
	let mut risk_bands: Vec<_> = (0..5)
		.map(|i| RiskBand {
			lower_inclusive: [0.0, 0.2, 0.4, 0.6, 0.8][i],
			upper: [0.2, 0.4, 0.6, 0.8, 1.0][i],
			upper_inclusive: i == 4,
			observations: 0,
			explicit_labels: 0,
			human_denials: 0,
			empirical_human_denial_rate: Rate::new(0, 0),
		})
		.collect();
	for sample in samples {
		let label = sample.settlement.label();
		labels += u64::from(label.is_some());
		match sample.settlement {
			SettlementClass::Rule => exclusions.rule += 1,
			SettlementClass::Auto => exclusions.auto += 1,
			SettlementClass::Bypass => exclusions.bypass += 1,
			SettlementClass::Expired => exclusions.expired += 1,
			SettlementClass::Cancelled => exclusions.cancelled += 1,
			SettlementClass::Revoked => exclusions.revoked += 1,
			SettlementClass::Pending => exclusions.pending += 1,
			_ => {}
		}
		match &sample.outcome {
			None => exclusions.missing_observations += 1,
			Some(ObservationOutcome::Pending) => exclusions.pending_observations += 1,
			Some(ObservationOutcome::Unavailable { .. }) => {
				exclusions.unavailable_observations += 1
			}
			Some(ObservationOutcome::Available { verdict }) => {
				available += 1;
				match (verdict, label) {
					(Verdict::Allowed, Some(HumanLabel::Allow)) => {
						confusion.shadow_allow_human_allow += 1
					}
					(Verdict::Allowed, Some(HumanLabel::Deny)) => {
						confusion.shadow_allow_human_deny += 1
					}
					(Verdict::Blocked(_), Some(HumanLabel::Allow)) => {
						confusion.shadow_block_human_allow += 1
					}
					(Verdict::Blocked(_), Some(HumanLabel::Deny)) => {
						confusion.shadow_block_human_deny += 1
					}
					_ => {}
				}
				if let Some(probability) = sample.maximum_category_probability {
					let index = risk_bands
						.iter()
						.position(|band| probability.value() < band.upper || band.upper_inclusive)
						.unwrap_or(4);
					let band = &mut risk_bands[index];
					band.observations += 1;
					band.explicit_labels += u64::from(label.is_some());
					band.human_denials += u64::from(label == Some(HumanLabel::Deny));
				}
			}
		}
	}
	for band in &mut risk_bands {
		band.empirical_human_denial_rate = Rate::new(band.human_denials, band.explicit_labels);
	}
	let aa = confusion.shadow_allow_human_allow;
	let ad = confusion.shadow_allow_human_deny;
	let ba = confusion.shadow_block_human_allow;
	let bd = confusion.shadow_block_human_deny;
	let comparable = aa + ad + ba + bd;
	let total = samples.len() as u64;
	ReportMetrics {
		total_approvals: total,
		available_observations: available,
		explicit_labels: labels,
		comparable,
		confusion,
		exclusions,
		agreement: Rate::new(aa + bd, comparable),
		false_allow_among_human_denials: Rate::new(ad, ad + bd),
		false_allow_among_shadow_allows: Rate::new(ad, aa + ad),
		false_block_among_human_approvals: Rate::new(ba, aa + ba),
		availability: Rate::new(available, total),
		label_coverage: Rate::new(labels, total),
		comparable_coverage: Rate::new(comparable, total),
		risk_bands,
	}
}
#[cfg(test)]
mod tests;
