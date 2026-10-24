//! A bytea primary key with the printable identity required by the ORM.
use reinhardt::db::orm::{DatabaseField, FieldCodecContext, FieldCodecError};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ByteKey(pub Vec<u8>);

impl From<Vec<u8>> for ByteKey {
	fn from(value: Vec<u8>) -> Self {
		Self(value)
	}
}

impl std::fmt::Display for ByteKey {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		for byte in &self.0 {
			write!(formatter, "{byte:02x}")?;
		}
		Ok(())
	}
}

impl DatabaseField for ByteKey {
	type Storage = Vec<u8>;
	fn encode_database(&self) -> Result<Self::Storage, FieldCodecError> {
		Ok(self.0.clone())
	}
	fn decode_database(
		value: Self::Storage,
		_context: &FieldCodecContext,
	) -> Result<Self, FieldCodecError> {
		Ok(Self(value))
	}
}
