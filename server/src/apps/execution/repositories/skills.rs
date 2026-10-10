//! Native skill storage retains row locks, immutable objects and the current Run scope.
use super::{
	capability_records::{domain, native},
	files::Scope,
};
use crate::apps::execution::capabilities::{
	serializers::{contracts::FileEntry, skills::Pinned as NativePinned},
	services::{records, sessions},
};
use crate::{Result as NativeResult, store::Store};
use aidash_application::{
	Error, Result,
	ports::capabilities::skills::{Limits, SkillHeadroom, SkillScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::MountedFile, records::Record, skills::Pinned},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::Value;
use uuid::Uuid;
fn missing() -> Error {
	Error::External("skill repository scope invariant".into())
}
impl From<NativePinned> for Pinned {
	fn from(v: NativePinned) -> Self {
		Self {
			metadata: v.metadata,
			loaded: v.loaded,
			instruction_json_len: v.instruction_json_len,
			files: v.files.into_iter().map(Into::into).collect(),
		}
	}
}
#[async_trait]
impl SkillScope for Scope<'_> {
	fn skill_limits(&self) -> Result<Limits> {
		let l = &self.store.ok_or_else(missing)?.capabilities.0.limits;
		Ok(Limits {
			files: l.skill_files,
			bytes: l.skill_bytes,
		})
	}
	async fn read_skill_file(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.read(self.access, &FileEntry::from(file.clone()))
			.await
			.map_err(Into::into)
	}
	async fn put_skill(&mut self, area: Uuid, bytes: &[u8]) -> Result<(Uuid, String)> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.put(self.access, Some(area), "skill", bytes)
			.await
			.map_err(Into::into)
	}
	async fn skill_record(&mut self, run: Uuid) -> Result<Record> {
		records::get(self.access, run, "skills")
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn insert_skills(&mut self, run: Uuid, area: Uuid, data: Value) -> Result<()> {
		records::insert(self.access, run, Some(area), "skills", "pinned", data, None)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn update_skills(&mut self, record: &mut Record) -> Result<()> {
		let mut row = native(record);
		let result = records::update(self.access, &mut row).await;
		*record = domain(row);
		result.map_err(Into::into)
	}
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()> {
		let current = self.run.ok_or_else(missing)?;
		if current.id != run.id {
			return Err(missing());
		};
		sessions::context_authority(self.access, current)
			.await
			.map_err(Into::into)
	}
}
pub(crate) struct Headroom<'a> {
	pub(crate) store: &'a Store,
}
#[async_trait]
impl SkillHeadroom for Headroom<'_> {
	async fn pinned_data(&mut self, id: Uuid) -> Result<Option<Value>> {
		let result: NativeResult<Option<Value>> = async {
			let store = self.store;
			let run = RunId { id };

			let data: Option<Value> = {
				let query_bind_1 = run.id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("data"))
						.from(Alias::new("core_records"))
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("skills")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&store.pool)
				.await?
			};
			Ok(data)
		}
		.await;
		result.map_err(Into::into)
	}
}
/// Current revision of a Run's pinned Skill record. A plain read: reuse of the
/// Skill context it keys is rechecked against current authority.
pub(crate) async fn revision(store: &Store, run: Uuid) -> Result<Option<i64>> {
	let revision: NativeResult<Option<i64>> = async {
		crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("revision"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run)))
				.and_where(Expr::col(Alias::new("kind")).eq(Expr::value("skills")))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&store.pool)
		.await
	}
	.await;
	revision.map_err(Into::into)
}
struct RunId {
	id: Uuid,
}
