//! Explicit conversion keeps native row codecs out of capability business state.
use crate::apps::execution::capabilities::serializers::records::Record as NativeRecord;
use aidash_domain::capabilities::records::Record;
pub(crate) fn domain(record: NativeRecord) -> Record {
	Record {
		id: record.id,
		tenant: record.tenant,
		owner: record.owner,
		area_id: record.area_id,
		kind: record.kind,
		state: record.state,
		revision: record.revision,
		data: record.data,
		expires_at: record.expires_at,
	}
}
pub(crate) fn native(record: &Record) -> NativeRecord {
	NativeRecord {
		id: record.id,
		tenant: record.tenant.clone(),
		owner: record.owner.clone(),
		area_id: record.area_id,
		kind: record.kind.clone(),
		state: record.state.clone(),
		revision: record.revision,
		data: record.data.clone(),
		expires_at: record.expires_at,
	}
}
