//! Immutable draft revision provenance and completed behavioral evidence.
use super::{AgentDraftRegistration, AgentTestSession};
use crate::apps::registry::serializers::contracts::Entry;
use crate::apps::registry::workbench::serializers::contracts::Draft;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{Alias, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder};
use uuid::Uuid;

impl AgentDraftRegistration {
	pub(crate) async fn for_agent(
		tx: &mut dyn TransactionExecutor,
		agent: &str,
		version: &str,
		limit: usize,
	) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_agent_id().eq(agent))
			.filter(Self::field_version().eq(version))
			.order_by(&["-registered_at", "-revision", "draft_id"])
			.limit(limit)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?)
	}

	pub(crate) async fn page(
		tx: &mut dyn TransactionExecutor,
		draft: Uuid,
		agent: &str,
	) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_draft_id().eq(draft))
			.filter(Self::field_agent_id().eq(agent))
			.order_by(&["-registered_at", "-revision", "draft_id"])
			.limit(100)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?)
	}

	pub(crate) async fn behavioral_evidence(
		tx: &mut dyn TransactionExecutor,
		draft: &Draft,
		entry: &Entry,
	) -> Result<bool> {
		let previous = Self::objects()
			.filter(Self::field_agent_id().eq(&entry.id))
			.filter(Self::field_version().eq(&entry.version))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.next();
		if let Some(previous) = previous {
			if previous.draft_id() != draft.id || previous.revision != draft.revision {
				return Err(Error::Conflict("version is already registered from another draft revision; choose a new semantic version".into()));
			}
			return Ok(previous.behavioral_tested);
		}
		Ok(!AgentTestSession::objects()
			.filter(AgentTestSession::field_draft_id().eq(draft.id))
			.filter(AgentTestSession::field_revision().eq(draft.revision))
			.filter(AgentTestSession::field_status().eq("completed"))
			.limit(1)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.is_empty())
	}

	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		draft: &Draft,
		entry: &Entry,
		actor: &str,
		behavioral_tested: bool,
	) -> Result<()> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(
				[
					"draft_id",
					"revision",
					"agent_id",
					"version",
					"actor",
					"release_notes",
					"source_id",
					"source_version",
					"behavioral_tested",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(draft.id),
				IntoValue::into_value(draft.revision),
				IntoValue::into_value(&entry.id),
				IntoValue::into_value(&entry.version),
				IntoValue::into_value(actor),
				IntoValue::into_value(&draft.release_notes),
				IntoValue::into_value(draft.source_id.clone()),
				IntoValue::into_value(draft.source_version.clone()),
				IntoValue::into_value(behavioral_tested),
			])
			.on_conflict(
				OnConflict::columns([Alias::new("draft_id"), Alias::new("revision")])
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

use reinhardt::query::IntoValue;
