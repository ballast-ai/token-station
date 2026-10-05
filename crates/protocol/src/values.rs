use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The longest key a [`ComponentValues`] map admits, in bytes.
pub const MAX_COMPONENT_VALUE_KEY_BYTES: usize = 64;

/// The longest value a [`ComponentValues`] map admits, in bytes.
pub const MAX_COMPONENT_VALUE_BYTES: usize = 4096;

/// Non-secret string values the host passes to a `provider-adapter` (0.5.0).
///
/// [`crate::ProviderConfig::declared`] and [`crate::ChatRequest::host_values`]
/// use this type. Every entry satisfies one grammar, checked on construction
/// and on deserialization, so a component and both hosts agree on it:
///
/// - A key is 1 to [`MAX_COMPONENT_VALUE_KEY_BYTES`] bytes of `a-z`, `0-9`
///   and `_`.
/// - A value is 1 to [`MAX_COMPONENT_VALUE_BYTES`] bytes of printable ASCII,
///   space included.
///
/// No control byte or non-ASCII byte can reach a header, a URL or a log through
/// this map. An absent value is an absent key.
///
/// The grammar cannot detect a secret. Which keys a component receives, and
/// that none of them holds a secret, stays the admitting layer's rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<String, String>",
    into = "BTreeMap<String, String>"
)]
pub struct ComponentValues(BTreeMap<String, String>);

impl ComponentValues {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key` to `value` and returns the previous value.
    ///
    /// # Errors
    ///
    /// Returns [`ComponentValueError`] when the key or the value breaks the
    /// grammar. The map does not change.
    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Option<String>, ComponentValueError> {
        let key = key.into();
        let value = value.into();
        check_entry(&key, &value)?;
        Ok(self.0.insert(key, value))
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.0.remove(key)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

impl TryFrom<BTreeMap<String, String>> for ComponentValues {
    type Error = ComponentValueError;

    fn try_from(map: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        for (key, value) in &map {
            check_entry(key, value)?;
        }
        Ok(Self(map))
    }
}

impl From<ComponentValues> for BTreeMap<String, String> {
    fn from(values: ComponentValues) -> Self {
        values.0
    }
}

fn check_entry(key: &str, value: &str) -> Result<(), ComponentValueError> {
    let key_is_valid = (1..=MAX_COMPONENT_VALUE_KEY_BYTES).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if !key_is_valid {
        return Err(ComponentValueError {
            key: key.to_owned(),
            reason: ComponentValueReason::Key,
        });
    }
    let value_is_valid = (1..=MAX_COMPONENT_VALUE_BYTES).contains(&value.len())
        && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte));
    if !value_is_valid {
        return Err(ComponentValueError {
            key: key.to_owned(),
            reason: ComponentValueReason::Value,
        });
    }
    Ok(())
}

/// A [`ComponentValues`] entry broke the grammar.
///
/// It names the key and never contains the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentValueError {
    key: String,
    reason: ComponentValueReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentValueReason {
    Key,
    Value,
}

impl ComponentValueError {
    /// The key of the refused entry, as written.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Whether the key, rather than the value, broke the grammar.
    #[must_use]
    pub fn is_key_invalid(&self) -> bool {
        self.reason == ComponentValueReason::Key
    }
}

impl fmt::Display for ComponentValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            ComponentValueReason::Key => write!(
                f,
                "component value key `{}` is not 1 to {MAX_COMPONENT_VALUE_KEY_BYTES} bytes of `a-z`, `0-9` and `_`",
                self.key.escape_debug()
            ),
            ComponentValueReason::Value => write!(
                f,
                "the value of component value `{}` is not 1 to {MAX_COMPONENT_VALUE_BYTES} bytes of printable ASCII",
                self.key
            ),
        }
    }
}

impl Error for ComponentValueError {}

#[cfg(test)]
mod tests {
    use super::{ComponentValues, MAX_COMPONENT_VALUE_BYTES, MAX_COMPONENT_VALUE_KEY_BYTES};

    #[test]
    fn insert_admits_the_grammar_and_returns_the_previous_value() {
        let mut values = ComponentValues::new();
        assert_eq!(values.insert("profile_arn", "arn:aws:x"), Ok(None));
        assert_eq!(
            values.insert("profile_arn", "arn:aws:y"),
            Ok(Some("arn:aws:x".to_owned()))
        );
        assert_eq!(values.get("profile_arn"), Some("arn:aws:y"));
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn insert_refuses_a_bad_key_or_value_and_leaves_the_map_unchanged() {
        let long_key = "k".repeat(MAX_COMPONENT_VALUE_KEY_BYTES + 1);
        let long_value = "v".repeat(MAX_COMPONENT_VALUE_BYTES + 1);
        let mut values = ComponentValues::new();
        values.insert("kept", "1").expect("valid entry");

        for (key, value, key_is_invalid) in [
            ("", "v", true),
            ("Account_Id", "v", true),
            ("account-id", "v", true),
            (long_key.as_str(), "v", true),
            ("account_id", "", false),
            ("account_id", "a\r\nx-injected: 1", false),
            ("account_id", "caf\u{e9}", false),
            ("account_id", "tab\there", false),
            ("account_id", long_value.as_str(), false),
        ] {
            let error = values.insert(key, value).expect_err("must be refused");
            assert_eq!(error.key(), key);
            assert_eq!(
                error.is_key_invalid(),
                key_is_invalid,
                "{key:?} / {value:?}"
            );
            assert!(
                !error.to_string().contains("x-injected"),
                "an error never repeats the value"
            );
        }
        assert_eq!(values.len(), 1);
        assert_eq!(values.get("kept"), Some("1"));
    }

    #[test]
    fn the_largest_value_and_key_are_admitted() {
        let mut values = ComponentValues::new();
        let key = "k".repeat(MAX_COMPONENT_VALUE_KEY_BYTES);
        let value = " ~".repeat(MAX_COMPONENT_VALUE_BYTES / 2);
        assert_eq!(values.insert(key.as_str(), value.as_str()), Ok(None));
    }
}
