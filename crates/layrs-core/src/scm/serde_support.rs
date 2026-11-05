use serde::de::{Deserialize, Deserializer};

/// Deserializes a present JSON field as `Some(value)`, preserving an explicit
/// JSON `null` as `Some(None)`. With `#[serde(default)]`, absence remains
/// `None`, so callers can enforce required-nullable semantics.
pub(crate) fn deserialize_present_nullable<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
