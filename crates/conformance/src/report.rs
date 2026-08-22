//! What the suite decided, in a form a registry can store and act on.
//!
//! A report is not a pass/fail bit. The architecture requires a refused package
//! to be kept as a draft with a recorded reason, and requires an upgrade or a
//! canary to be gated on the same evidence. So every check that ran is named,
//! every failure carries the case that produced it, and [`Check`] is a closed
//! enumeration rather than a string — an operator filters on it, and a `-v2`
//! suite that adds a check has to say so in the type.

use std::fmt;

/// One property the suite asserts of an adapter.
///
/// These do not map one-to-one onto the rows of the architecture's acceptance
/// table. The request conversion, response conversion, and error mapping rows are all
/// [`Check::FixtureMatch`] — they differ only in which fixture family produced
/// the case, which the [`Outcome::case`] name records. The rows that need their
/// own check are the ones a fixture comparison cannot express.
///
/// The table's security row is deliberately absent. No network, no file system,
/// and the memory and time bounds are properties of the sandbox the runtime
/// builds, not of anything an adapter can be asked to compute. `plugin-runtime`
/// enforces them; a fixture that claimed to would be theatre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    /// The pack carries at least one case for every fixture family.
    ///
    /// Without it an adapter passes by shipping nothing. The audit table in the
    /// architecture requires fixtures to cover request translation, streaming,
    /// error mapping and unknown fields; this is that requirement, enforced.
    Coverage,
    /// The adapter's output equals the fixture's expected output, byte for byte
    /// after canonical serialization.
    FixtureMatch,
    /// The same input, invoked twice, produced the same output.
    ///
    /// Catches map iteration order, clocks and randomness. A non-deterministic
    /// adapter makes a conformance pass meaningless, because the run that
    /// admitted it is not the run the host will get.
    Determinism,
    /// An input carrying a field this ABI version does not model was tolerated.
    ///
    /// A `v1` adapter meeting a `v2` peer must degrade, not fail.
    UnknownFieldTolerance,
}

impl Check {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coverage => "coverage",
            Self::FixtureMatch => "fixture_match",
            Self::Determinism => "determinism",
            Self::UnknownFieldTolerance => "unknown_field_tolerance",
        }
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    /// Why, in terms an operator can act on without reading the fixture.
    Failed(String),
}

/// One check, against one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub check: Check,
    /// The fixture case, e.g. `provider.error.rate-limit`. [`Check::Coverage`]
    /// names the missing family instead.
    pub case: String,
    pub verdict: Verdict,
}

impl Outcome {
    #[must_use]
    pub fn passed(check: Check, case: impl Into<String>) -> Self {
        Self {
            check,
            case: case.into(),
            verdict: Verdict::Passed,
        }
    }

    #[must_use]
    pub fn failed(check: Check, case: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check,
            case: case.into(),
            verdict: Verdict::Failed(detail.into()),
        }
    }

    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(self.verdict, Verdict::Failed(_))
    }

    /// Why it failed; empty when it passed.
    #[must_use]
    pub fn detail(&self) -> &str {
        match &self.verdict {
            Verdict::Passed => "",
            Verdict::Failed(detail) => detail,
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.verdict {
            Verdict::Passed => write!(f, "{}: {} passed", self.case, self.check),
            Verdict::Failed(detail) => write!(f, "{}: {} failed: {detail}", self.case, self.check),
        }
    }
}

/// The verdict on one adapter, against one suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    suite: &'static str,
    outcomes: Vec<Outcome>,
}

impl Report {
    #[must_use]
    pub fn new(suite: &'static str, outcomes: Vec<Outcome>) -> Self {
        Self { suite, outcomes }
    }

    /// The suite that produced this, e.g. `agent-protocol-v1`. Matches the
    /// `conformance.required_suite` the manifest declared.
    #[must_use]
    pub fn suite(&self) -> &'static str {
        self.suite
    }

    #[must_use]
    pub fn outcomes(&self) -> &[Outcome] {
        &self.outcomes
    }

    pub fn failures(&self) -> impl Iterator<Item = &Outcome> {
        self.outcomes.iter().filter(|outcome| outcome.is_failure())
    }

    /// Whether the package may enter the runtime registry.
    #[must_use]
    pub fn is_passing(&self) -> bool {
        self.failures().next().is_none()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{}: {} checks, {} failed",
            self.suite,
            self.outcomes.len(),
            self.failures().count()
        )?;
        for failure in self.failures() {
            writeln!(f, "  {failure}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Check, Outcome, Report, Verdict};

    #[test]
    fn a_report_with_no_outcomes_is_not_a_pass_by_accident() {
        // It is vacuously passing, which is exactly why `Coverage` exists: an
        // empty fixture pack fails before it can produce an empty report.
        let report = Report::new("agent-protocol-v1", Vec::new());

        assert!(report.is_passing());
        assert_eq!(report.outcomes().len(), 0);
    }

    #[test]
    fn a_single_failure_refuses_the_package_and_says_why() {
        let report = Report::new(
            "agent-protocol-v1",
            vec![
                Outcome::passed(Check::FixtureMatch, "agent.normalize.chat"),
                Outcome::failed(
                    Check::Determinism,
                    "agent.normalize.chat",
                    "second invocation rendered a different document",
                ),
            ],
        );

        assert!(!report.is_passing());
        assert_eq!(report.failures().count(), 1);
        assert!(format!("{report}").contains("different document"));
    }

    #[test]
    fn outcome_renders_the_case_and_the_reason() {
        let outcome = Outcome::failed(Check::Determinism, "agent.stream.delta", "differed");

        assert_eq!(
            outcome.to_string(),
            "agent.stream.delta: determinism failed: differed"
        );
        assert_eq!(outcome.verdict, Verdict::Failed("differed".to_owned()));
    }
}
