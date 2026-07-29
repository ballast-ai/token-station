//! The fixture pack a plugin package ships, and how it is read.
//!
//! A case is a pair of files inside the directory the manifest's
//! `conformance.fixtures` names:
//!
//! ```text
//! provider.request.chat.input.json
//! provider.request.chat.expected.json
//! ```
//!
//! The name is `<kind>.<family>.<case>`. `kind` must match the suite, `family`
//! selects which adapter function the input is fed to, and `case` is free. A
//! case name is required even when a family has only one case: the alternative
//! is a pack where adding the second case renames the first, and fixture names
//! appear in conformance reports that outlive the pack.
//!
//! Inputs are the Canonical IR, not the provider's wire format. A fixture that
//! could hold a credential would be a way to smuggle one past the type system,
//! so the IR's own boundaries — [`token_station_protocol::HeaderDigest`],
//! [`token_station_protocol::SafeHeaders`],
//! [`token_station_protocol::ProviderEndpoint`] — re-apply on deserialization
//! here exactly as they do anywhere else.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

const MAX_FIXTURE_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Which adapter function a fixture family exercises.
pub trait Family: Copy + Eq + fmt::Debug + Sized + 'static {
    /// The `kind` token every file in a pack for this family must carry.
    const KIND: &'static str;
    /// Every family, so [`Check::Coverage`](crate::Check) can name a missing one.
    const ALL: &'static [Self];

    fn token(self) -> &'static str;

    /// Where inside the case's input JSON an unknown field may be injected, as a
    /// JSON Pointer. `None` when the family's input has nowhere forward-
    /// compatible to put one.
    ///
    /// Not every input can host one. [`token_station_protocol::StreamChunk`] is
    /// an opaque string, and [`token_station_protocol::StreamEvent`] carries no
    /// `extensions` on purpose — an event is consumed once and never re-
    /// serialized, so there is nothing for a stray field to survive into.
    fn unknown_field_pointer(self) -> Option<&'static str>;

    fn parse(token: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|f| f.token() == token)
    }
}

/// The families of a `provider-adapter` pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFamily {
    /// `ProviderConfig` -> `list<ModelCapability>`.
    Capabilities,
    /// `{ provider_config, chat_request }` -> `HttpRequestDescriptor`.
    Request,
    /// `HttpResponseParts` -> `ChatResponse`.
    Response,
    /// `{ chunks: [string] }` -> `list<StreamEvent>`.
    Stream,
    /// `HttpResponseParts` -> `ErrorEnvelope`.
    Error,
}

impl Family for ProviderFamily {
    const KIND: &'static str = "provider";
    const ALL: &'static [Self] = &[
        Self::Capabilities,
        Self::Request,
        Self::Response,
        Self::Stream,
        Self::Error,
    ];

    fn token(self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Request => "request",
            Self::Response => "response",
            Self::Stream => "stream",
            Self::Error => "error",
        }
    }

    fn unknown_field_pointer(self) -> Option<&'static str> {
        match self {
            // `ChatRequest` and `HttpResponseParts` both carry `extensions`.
            Self::Request => Some("/chat_request"),
            Self::Capabilities | Self::Response | Self::Error => Some(""),
            Self::Stream => None,
        }
    }
}

/// The families of an `agent-adapter` pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFamily {
    /// `AgentRequestEnvelope` -> `ChatRequest`.
    Normalize,
    /// `AgentRequestEnvelope` -> `list<AgentHint>`.
    Hint,
    /// `{ context, response }` -> the entry protocol's response document.
    Render,
    /// `{ context, events: [StreamEvent] }` -> the entry protocol's chunks.
    Stream,
    /// `{ context, error }` -> the entry protocol's error document.
    Error,
}

impl Family for AgentFamily {
    const KIND: &'static str = "agent";
    const ALL: &'static [Self] = &[
        Self::Normalize,
        Self::Hint,
        Self::Render,
        Self::Stream,
        Self::Error,
    ];

    fn token(self) -> &'static str {
        match self {
            Self::Normalize => "normalize",
            Self::Hint => "hint",
            Self::Render => "render",
            Self::Stream => "stream",
            Self::Error => "error",
        }
    }

    fn unknown_field_pointer(self) -> Option<&'static str> {
        match self {
            Self::Normalize | Self::Hint => Some(""),
            Self::Render => Some("/response"),
            Self::Error => Some("/error"),
            // A `StreamEvent` has no `extensions` to receive one.
            Self::Stream => None,
        }
    }
}

/// One input, and what the adapter must produce from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Case<F> {
    /// The full `<kind>.<family>.<case>` name, as it appears in a report.
    pub name: String,
    pub family: F,
    pub input: Value,
    pub expected: Value,
}

/// Every case a plugin package ships, in a stable order.
#[derive(Debug, Clone, PartialEq)]
pub struct FixturePack<F> {
    cases: Vec<Case<F>>,
}

impl<F: Family> FixturePack<F> {
    /// Reads every `*.input.json` in `directory` and pairs it with its expected
    /// output.
    ///
    /// Files that do not name this pack's `kind` are ignored rather than
    /// refused: an `agent-adapter` and a `provider-adapter` shipped in one
    /// package may share a fixtures directory.
    ///
    /// # Errors
    ///
    /// Returns the first [`FixtureError`] found. A pack that does not load is
    /// not a pack that fails conformance — it is a malformed package, and the
    /// registry has to tell those apart.
    pub fn load(directory: &Path) -> Result<Self, FixtureError> {
        let entries = fs::read_dir(directory).map_err(|source| FixtureError::Unreadable {
            path: directory.to_path_buf(),
            detail: source.to_string(),
        })?;

        let mut inputs = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|source| FixtureError::Unreadable {
                    path: directory.to_path_buf(),
                    detail: source.to_string(),
                })?
                .path();

            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(stem) = file_name.strip_suffix(".input.json") else {
                continue;
            };
            inputs.push((stem.to_owned(), path));
        }
        inputs.sort_by(|left, right| left.0.cmp(&right.0));

        let mut cases = Vec::new();
        for (stem, input_path) in inputs {
            let Some(family) = family_of::<F>(&stem)? else {
                continue;
            };
            let expected_path = directory.join(format!("{stem}.expected.json"));
            cases.push(Case {
                input: read_json(&input_path, &stem)?,
                expected: read_json(&expected_path, &stem)?,
                family,
                name: stem,
            });
        }

        Ok(Self { cases })
    }

    #[must_use]
    pub fn from_cases(cases: Vec<Case<F>>) -> Self {
        Self { cases }
    }

    #[must_use]
    pub fn cases(&self) -> &[Case<F>] {
        &self.cases
    }

    /// Families with no case. Empty is the only passing answer.
    #[must_use]
    pub fn missing_families(&self) -> Vec<F> {
        F::ALL
            .iter()
            .copied()
            .filter(|family| !self.cases.iter().any(|case| case.family == *family))
            .collect()
    }
}

/// `<kind>.<family>.<case>` -> the family, or `None` when the file belongs to
/// the other role's pack.
fn family_of<F: Family>(stem: &str) -> Result<Option<F>, FixtureError> {
    let mut segments = stem.splitn(3, '.');
    let (Some(kind), Some(token), Some(case)) = (segments.next(), segments.next(), segments.next())
    else {
        return Err(FixtureError::MalformedName {
            name: stem.to_owned(),
        });
    };

    if kind != F::KIND {
        return Ok(None);
    }
    if case.is_empty() {
        return Err(FixtureError::MalformedName {
            name: stem.to_owned(),
        });
    }

    F::parse(token)
        .ok_or_else(|| FixtureError::UnknownFamily {
            name: stem.to_owned(),
            family: token.to_owned(),
        })
        .map(Some)
}

fn read_json(path: &Path, case: &str) -> Result<Value, FixtureError> {
    let file = fs::File::open(path).map_err(|source| FixtureError::Unreadable {
        path: path.to_path_buf(),
        detail: source.to_string(),
    })?;
    let metadata = file.metadata().map_err(|source| FixtureError::Unreadable {
        path: path.to_path_buf(),
        detail: source.to_string(),
    })?;
    if metadata.len() > MAX_FIXTURE_FILE_BYTES {
        return Err(FixtureError::Unreadable {
            path: path.to_path_buf(),
            detail: format!("fixture exceeds the {MAX_FIXTURE_FILE_BYTES} byte limit"),
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_FIXTURE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| FixtureError::Unreadable {
            path: path.to_path_buf(),
            detail: source.to_string(),
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FIXTURE_FILE_BYTES {
        return Err(FixtureError::Unreadable {
            path: path.to_path_buf(),
            detail: format!("fixture exceeds the {MAX_FIXTURE_FILE_BYTES} byte limit"),
        });
    }
    let source = String::from_utf8(bytes).map_err(|source| FixtureError::Unreadable {
        path: path.to_path_buf(),
        detail: source.to_string(),
    })?;
    serde_json::from_str(&source).map_err(|source| FixtureError::NotJson {
        case: case.to_owned(),
        detail: source.to_string(),
    })
}

/// Why a fixture pack could not be read.
///
/// Distinct from a conformance failure. A pack that fails the suite is a plugin
/// that does not work; a pack that will not load is a package that was built
/// wrong, and the registry stores a different reason for each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureError {
    Unreadable {
        path: PathBuf,
        detail: String,
    },
    /// Not `<kind>.<family>.<case>.input.json`, or missing its `.expected.json`.
    MalformedName {
        name: String,
    },
    UnknownFamily {
        name: String,
        family: String,
    },
    NotJson {
        case: String,
        detail: String,
    },
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, detail } => {
                write!(f, "cannot read `{}`: {detail}", path.display())
            }
            Self::MalformedName { name } => write!(
                f,
                "fixture `{name}` is not named `<kind>.<family>.<case>.input.json`"
            ),
            Self::UnknownFamily { name, family } => {
                write!(f, "fixture `{name}` names no such family `{family}`")
            }
            Self::NotJson { case, detail } => write!(f, "fixture `{case}` is not JSON: {detail}"),
        }
    }
}

impl Error for FixtureError {}

#[cfg(test)]
mod tests {
    use super::{AgentFamily, Family, FixtureError, FixturePack, ProviderFamily, family_of};
    use std::fs;

    #[test]
    fn a_file_from_the_other_role_is_skipped_not_refused() {
        // One package, two roles, one fixtures directory.
        assert_eq!(
            family_of::<ProviderFamily>("agent.normalize.chat"),
            Ok(None)
        );
        assert_eq!(family_of::<AgentFamily>("provider.request.chat"), Ok(None));

        assert_eq!(
            family_of::<AgentFamily>("agent.normalize.chat"),
            Ok(Some(AgentFamily::Normalize))
        );
    }

    #[test]
    fn a_family_this_suite_does_not_know_is_refused_by_name() {
        assert_eq!(
            family_of::<ProviderFamily>("provider.telemetry.chat"),
            Err(FixtureError::UnknownFamily {
                name: "provider.telemetry.chat".to_owned(),
                family: "telemetry".to_owned(),
            })
        );
    }

    #[test]
    fn a_case_name_is_required() {
        assert_eq!(
            family_of::<ProviderFamily>("provider.request"),
            Err(FixtureError::MalformedName {
                name: "provider.request".to_owned(),
            })
        );
    }

    #[test]
    fn every_family_round_trips_through_its_token() {
        for family in ProviderFamily::ALL {
            assert_eq!(ProviderFamily::parse(family.token()), Some(*family));
        }
        for family in AgentFamily::ALL {
            assert_eq!(AgentFamily::parse(family.token()), Some(*family));
        }
    }

    #[test]
    fn stream_families_host_no_unknown_field() {
        assert_eq!(ProviderFamily::Stream.unknown_field_pointer(), None);
        assert_eq!(AgentFamily::Stream.unknown_field_pointer(), None);
    }

    #[test]
    fn a_single_fixture_file_cannot_allocate_without_bound() {
        let directory = std::env::temp_dir().join(format!(
            "token-station-oversized-fixture-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch creates");
        fs::write(
            directory.join("agent.normalize.large.input.json"),
            vec![b' '; 2 * 1024 * 1024 + 1],
        )
        .expect("fixture writes");

        let error = FixturePack::<AgentFamily>::load(&directory)
            .expect_err("oversized fixture must be refused before JSON parsing");
        assert!(error.to_string().contains("limit"), "{error}");
        fs::remove_dir_all(directory).ok();
    }
}
