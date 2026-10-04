//! Factual inspection only. No Trust verdict or certification is inferred.
use super::*;
use crate::{authorization::access::NativeAccess, registry::EntityRef};
use reinhardt::injectable;

use crate::apps::execution::models::Run as RunRecord;
use crate::apps::registry::workbench::models::AgentTestSession;
use crate::apps::workspaces::models::Workspace;

pub(super) async fn require_inspection(
	tx: &mut dyn TransactionExecutor,
	actor: &Actor,
	reference: &EntityRef,
) -> Result<()> {
	aidash_application::registry::workbench::inspection::require(
		&mut crate::bootstrap::draft_authority_scope(tx, actor),
		reference,
	)
	.await
	.map_err(Into::into)
}

// Inspection and contained resource checks share one audit allocation lease.
// Opening another audited transaction while this one is live would wait on
// the inspection's own advisory lock.
enum InspectionLease {
	Operator(Box<dyn TransactionExecutor>),
	Subject(Box<NativeAccess>),
}

impl InspectionLease {
	async fn begin(f: &Federation, actor: &Actor) -> Result<Self> {
		match actor {
			Actor::Operator => Ok(Self::Operator(Box::new(PgTransactionExecutor::new(
				f.store.pool.begin().await?,
			)))),
			Actor::Subject(identity) => Ok(Self::Subject(Box::new(
				NativeAccess::begin(&f.store, identity).await?,
			))),
		}
	}

	fn tx(&mut self) -> &mut dyn TransactionExecutor {
		match self {
			Self::Operator(tx) => tx.as_mut(),
			Self::Subject(access) => access.tx.as_mut(),
		}
	}

	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		match self {
			Self::Operator(_) => Ok(true),
			Self::Subject(access) => {
				let result = async {
					let workspace = access.workspace(run.workspace_id).await?;
					if !access.decide(&workspace, "workspace.read").await? {
						return Ok(false);
					}
					access.run_visible(run).await
				}
				.await;
				match result {
					Err(Error::Forbidden) => Ok(false),
					other => other,
				}
			}
		}
	}

	async fn finish(self, inspection: Inspection) -> Result<Inspection> {
		match self {
			Self::Operator(tx) => {
				tx.commit().await?;
				Ok(inspection)
			}
			Self::Subject(access) => (*access).finish(Ok(inspection)).await,
		}
	}
}

async fn visible_runs(
	lease: &mut InspectionLease,
	reference: &EntityRef,
) -> Result<Vec<RunMetadata>> {
	let mut visible = Vec::new();
	let mut cursor: Option<(DateTime<Utc>, Uuid)> = None;
	loop {
		let rows =
			RunRecord::agent_page(lease.tx(), &reference.id, &reference.version, cursor).await?;
		let more = rows.len() == 501;
		cursor = rows.last().map(|run| (run.updated_at, run.id));
		for run in rows {
			if lease.run_visible(&run).await? {
				visible.push(run);
			}
			if visible.len() == 501 {
				break;
			}
		}
		if visible.len() == 501 || !more {
			break;
		}
	}
	Ok(visible)
}

async fn add_use(
	tx: &mut dyn TransactionExecutor,
	uses: &mut std::collections::BTreeMap<Uuid, WorkspaceUse>,
	run: &RunMetadata,
) -> Result<()> {
	let active = !run.phase().is_terminal();
	if let Some(existing) = uses.get_mut(&run.workspace_id) {
		existing.current |= active;
		if run.updated_at > existing.latest_run_at {
			existing.latest_run_at = run.updated_at;
		}
		return Ok(());
	}
	let title = Workspace::title_in(tx, run.workspace_id).await?;
	if let Some(title) = title {
		uses.insert(
			run.workspace_id,
			WorkspaceUse {
				workspace_id: run.workspace_id,
				title,
				current: active,
				latest_run_at: run.updated_at,
			},
		);
	}
	Ok(())
}

fn escape_html(value: &str) -> String {
	value
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
}

pub(crate) use crate::apps::registry::workbench::serializers::trust::ReportQuery;
pub use crate::apps::registry::workbench::serializers::trust::{
	Inspection, PermissionContext, PermissionInput, PermissionRow, Report, TestEvidence,
	WorkspaceUse,
};

use reinhardt::Response;

#[derive(Clone)]
pub struct TrustInspection {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_trust(#[inject] runtime: Federation) -> TrustInspection {
	TrustInspection { runtime }
}

impl TrustInspection {
	pub(crate) async fn inspect(
		&self,
		actor: Actor,
		(id, version): (String, String),
	) -> Result<Inspection> {
		let f = self.runtime.clone();
		let reference = EntityRef { id, version };
		let mut lease = InspectionLease::begin(&f, &actor).await?;
		require_inspection(lease.tx(), &actor, &reference).await?;
		let entry = admission::effective(lease.tx(), &reference.id, &reference.version).await?;
		if entry.kind != "agent" {
			return Err(Error::NotFound("agent version".into()));
		}
		let runs = visible_runs(&mut lease, &reference).await?;
		let truncated = runs.len() > 500;
		let mut uses: std::collections::BTreeMap<Uuid, WorkspaceUse> = Default::default();
		for run in runs.into_iter().take(500) {
			add_use(lease.tx(), &mut uses, &run).await?;
		}
		let mut test_evidence = Vec::new();
		let mut test_evidence_truncated = false;
		let registrations =
			AgentDraftRegistration::for_agent(lease.tx(), &reference.id, &reference.version, 1)
				.await?;
		for registration in registrations {
			let (draft_id, revision) = (registration.draft_id(), registration.revision);
			let draft = AgentDraft::read(lease.tx(), draft_id, false).await?;
			match authorize(lease.tx(), &actor, &draft, "agent_draft.read", true).await {
				Ok(()) => {}
				Err(Error::Forbidden) => continue,
				Err(error) => return Err(error),
			}
			let sessions =
				AgentTestSession::evidence_page(lease.tx(), draft_id, revision, 101).await?;
			test_evidence_truncated = sessions.len() > 100;
			for session in sessions.into_iter().take(100) {
				test_evidence.push(TestEvidence {
					session_id: session.id,
					draft_revision: revision,
					mode: session.scenario["mode"]
						.as_str()
						.unwrap_or("unknown")
						.into(),
					profile_id: session.scenario["profile_id"].as_str().map(str::to_owned),
					profile_revision: session.scenario["profile_revision"].as_i64(),
					status: session.status,
					usage: session.usage,
					created_at: session.created_at,
					expires_at: session.expires_at,
					expired_at: session.expired_at,
				});
			}
		}
		let inspection = Inspection {
			entry,
			source_node: f.config.node_id,
			observed_at: Utc::now(),
			workspaces: uses.into_values().collect(),
			usage_truncated: truncated,
			test_evidence,
			test_evidence_truncated,
			external_assessment_available: false,
		};
		lease.finish(inspection).await
	}
	pub(crate) async fn permission_context(
		&self,
		actor: Actor,
		(id, version): (String, String),
		input: PermissionInput,
	) -> Result<PermissionContext> {
		aidash_application::registry::workbench::permissions::inspect(
			&crate::bootstrap::workbench_permission_repository(&self.runtime, actor),
			EntityRef { id, version },
			input,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn report(
		&self,
		actor: Actor,
		(id, version): (String, String),
		query: ReportQuery,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let format = query.format.as_deref().unwrap_or("json");
		if !matches!(format, "json" | "html") {
			return Err(Error::Invalid("report format must be json or html".into()));
		}
		let inspection = self
			.inspect(actor.clone(), (id.clone(), version.clone()))
			.await?;
		let context_input = match &actor {
			Actor::Subject(identity) => {
				if query
					.tenant
					.as_ref()
					.is_some_and(|tenant| tenant != &identity.tenant)
					|| query
						.subject
						.as_ref()
						.is_some_and(|subject| subject != &identity.subject)
				{
					return Err(Error::Forbidden);
				}
				Some(PermissionInput {
					tenant: identity.tenant.clone(),
					subject: identity.subject.clone(),
					workspace_id: query.workspace_id,
				})
			}
			Actor::Operator => match (&query.tenant, &query.subject) {
				(Some(tenant), Some(subject)) => Some(PermissionInput {
					tenant: tenant.clone(),
					subject: subject.clone(),
					workspace_id: query.workspace_id,
				}),
				(None, None) if query.workspace_id.is_none() => None,
				_ => {
					return Err(Error::Invalid(
						"report permission context requires tenant and subject together".into(),
					));
				}
			},
		};
		let context = if let Some(input) = context_input {
			Some(
				self.permission_context(actor.clone(), (id.clone(), version.clone()), input)
					.await?,
			)
		} else {
			None
		};
		let mut incidents = super::incident::Incidents { runtime: f }
			.list(actor, (id, version))
			.await?;
		if !query.include_sensitive {
			for incident in &mut incidents {
				if let Some(copies) = incident.evidence.as_array_mut() {
					for copy in copies {
						if let Some(copy) = copy.as_object_mut() {
							copy.remove("content");
						}
					}
				}
			}
		}
		let report = Report {
			inspection,
			permission_context: context,
			incidents,
			exported_at: Utc::now(),
			note:
				"Factual connected-node snapshot; no Trust assessment or certification is provided."
					.into(),
		};
		if format == "json" {
			return Ok(Response::ok().with_json(&(report))?);
		}
		let title = escape_html(&format!(
			"Aidash agent report · {}@{}",
			report.inspection.entry.id, report.inspection.entry.version
		));
		let content = escape_html(&serde_json::to_string_pretty(&report)?);
		let html = format!(
			"<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:14px/1.5 system-ui,sans-serif;max-width:900px;margin:32px auto;color:#172236}}h1{{font-size:24px}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#f4f6fa;padding:20px;border:1px solid #dce2ec}}@media print{{body{{margin:0}}pre{{border:0;padding:0}}}}</style></head><body><h1>{title}</h1><p>Factual connected-node snapshot. No Trust assessment or certification is provided.</p><pre>{content}</pre></body></html>"
		);
		Ok(Response::ok()
			.with_body(html)
			.with_header("Content-Type", "text/html; charset=utf-8"))
	}
}

use crate::domain::RunMetadata;
