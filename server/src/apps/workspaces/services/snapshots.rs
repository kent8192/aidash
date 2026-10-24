//! Bound federation pages without truncating or repeating an individual record.
use crate::{Error, Result, domain::SnapshotPage};
use serde_json::Value;
use uuid::Uuid;

pub(crate) fn page(rows: Vec<(Uuid, Value)>, after: Option<Uuid>) -> Result<SnapshotPage> {
	let full = rows.len() == 32;
	let mut page = SnapshotPage {
		items: vec![],
		next: None,
	};
	let mut size = 128;
	let mut last = after;
	for (id, item) in rows {
		let item_size = serde_json::to_vec(&item)?.len() + 1;
		if size + item_size > 3_145_728 {
			if page.items.is_empty() {
				return Err(Error::Invalid(
					"individual workspace resource exceeds the 3 MiB federation page limit".into(),
				));
			}
			page.next = last;
			return Ok(page);
		}
		size += item_size;
		page.items.push(item);
		last = Some(id);
	}
	if full {
		page.next = last;
	}
	Ok(page)
}
