//! Approved HTTPS targets and durable outbound state transitions contain no I/O.
use super::{
	operations::{FileScope, MountedFile},
	records::Record,
};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::net::IpAddr;
use uuid::Uuid;
pub fn permitted_origin(url: &str, origins: &[String]) -> Result<(url::Url, String)> {
	let parsed = url::Url::parse(url).map_err(|_| Error::Invalid("INVALID_OUTBOUND_URL".into()))?;
	if url.len() > 4096
		|| parsed.scheme() != "https"
		|| !parsed.username().is_empty()
		|| parsed.password().is_some()
		|| parsed.fragment().is_some()
		|| parsed.port_or_known_default() != Some(443)
	{
		return Err(Error::Invalid(
			"outbound access requires an HTTPS URL without credentials on port 443".into(),
		));
	}
	let origin = parsed.origin().ascii_serialization();
	if !origins.contains(&origin) {
		return Err(Error::Conflict("OUTBOUND_OPERATOR_DENIED".into()));
	}
	Ok((parsed, origin))
}
pub fn public_ip(ip: IpAddr) -> bool {
	match ip {
		IpAddr::V4(ip) => {
			let [a, b, c, _] = ip.octets();
			!ip.is_unspecified()
				&& !ip.is_loopback()
				&& !ip.is_private()
				&& !ip.is_link_local()
				&& !ip.is_broadcast()
				&& !ip.is_multicast()
				&& a != 0 && a < 224
				&& !(a == 100 && (64..128).contains(&b))
				&& !(a == 192 && b == 0 && (c == 0 || c == 2))
				&& !(a == 198 && (b == 18 || b == 19 || b == 51 && c == 100))
				&& !(a == 203 && b == 0 && c == 113)
		}
		IpAddr::V6(ip) => {
			let s = ip.segments();
			// Only native global unicast; reject transition/tunnel and special
			// protocol/documentation ranges as well as mapped IPv4 addresses.
			(s[0] & 0xe000) == 0x2000
				&& !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
				&& s[0] != 0x2002
				&& !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
		}
	}
}
pub fn attempted(record: &mut Record, at: DateTime<Utc>, deadline: DateTime<Utc>) {
	record.state = "attempted".into();
	record.data["attempted_at"] = json!(at);
	record.data["attempt_deadline"] = json!(deadline);
}
pub fn completed(
	record: &mut Record,
	file_id: Uuid,
	digest: String,
	status: u16,
	bytes: &[u8],
	final_url: &str,
) {
	let entry = MountedFile {
		file_id,
		path: format!("outbound-{}.bin", record.id),
		digest,
		size: bytes.len() as u64,
		media_type: "application/octet-stream".into(),
		scope: FileScope::Working,
		provenance: json!({"kind":"outbound","operation_id":record.id}),
	};
	record.state = "completed".into();
	record.data["output_file"] = json!(entry);
	record.data["http_status"] = json!(status);
	record.data["final_url"] = json!(final_url);
}
pub fn failure_disclosure(code: &str) -> Value {
	json!({"error":{"code":code,"message":"Outbound execution stopped; earlier effects may have occurred.","retryable":false}})
}
#[cfg(test)]
mod tests;
