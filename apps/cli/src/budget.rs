//! Informational per-Agent budget evaluation.
//!
//! This module deliberately has no gateway/admission dependency: a budget can
//! produce warnings, never a routing or request permit decision.

use serde::{Deserialize, Serialize};

pub const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_LIMIT_MICROS: u64 = 9_000_000_000_000_000;

const fn default_warning_percent() -> u8 {
    80
}

const fn default_expiry_warning_days() -> u16 {
    7
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBudget {
    pub limit_micros: u64,
    #[serde(default = "default_warning_percent")]
    pub warning_percent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_end_ms: Option<u64>,
    #[serde(default = "default_expiry_warning_days")]
    pub expiry_warning_days: u16,
}

impl AgentBudget {
    /// Validates a display-only budget.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero/unsafe amount, invalid warning threshold,
    /// excessive expiry window, or reversed period.
    pub fn validate(&self) -> Result<(), String> {
        if self.limit_micros == 0 || self.limit_micros > MAX_LIMIT_MICROS {
            return Err(format!(
                "budget limit_micros must be between 1 and {MAX_LIMIT_MICROS}"
            ));
        }
        if !(1..=100).contains(&self.warning_percent) {
            return Err("budget warning_percent must be between 1 and 100".to_string());
        }
        if self.expiry_warning_days > 365 {
            return Err("budget expiry_warning_days must not exceed 365".to_string());
        }
        if self
            .period_start_ms
            .zip(self.period_end_ms)
            .is_some_and(|(start, end)| start >= end)
        {
            return Err("budget period_end_ms must be after period_start_ms".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetUsageLevel {
    Healthy,
    Approaching,
    Exceeded,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetExpiryLevel {
    None,
    Active,
    Expiring,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetEnforcement {
    ObserveOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BudgetStatus {
    pub agent_id: String,
    pub limit_micros: u64,
    pub used_micros: u64,
    pub remaining_micros: u64,
    pub warning_percent: u8,
    pub usage_percent: u16,
    pub unpriced_requests: u64,
    pub period_start_ms: Option<u64>,
    pub period_end_ms: Option<u64>,
    pub expiry_warning_days: u16,
    pub usage_level: BudgetUsageLevel,
    pub expiry_level: BudgetExpiryLevel,
    pub enforcement: BudgetEnforcement,
    pub routing_affected: bool,
}

impl BudgetStatus {
    #[must_use]
    pub fn evaluate(
        agent_id: &str,
        budget: &AgentBudget,
        used_micros: u64,
        unpriced_requests: u64,
        now_ms: u64,
    ) -> Self {
        let usage_percent_u64 = used_micros.saturating_mul(100) / budget.limit_micros.max(1);
        let usage_percent = u16::try_from(usage_percent_u64).unwrap_or(u16::MAX);
        let usage_level = if used_micros >= budget.limit_micros {
            BudgetUsageLevel::Exceeded
        } else if unpriced_requests > 0 {
            BudgetUsageLevel::Unknown
        } else if usage_percent >= u16::from(budget.warning_percent) {
            BudgetUsageLevel::Approaching
        } else {
            BudgetUsageLevel::Healthy
        };
        let expiry_level = match budget.period_end_ms {
            None => BudgetExpiryLevel::None,
            Some(end) if end <= now_ms => BudgetExpiryLevel::Expired,
            Some(end)
                if end.saturating_sub(now_ms)
                    <= u64::from(budget.expiry_warning_days).saturating_mul(DAY_MS) =>
            {
                BudgetExpiryLevel::Expiring
            }
            Some(_) => BudgetExpiryLevel::Active,
        };
        Self {
            agent_id: agent_id.to_string(),
            limit_micros: budget.limit_micros,
            used_micros,
            remaining_micros: budget.limit_micros.saturating_sub(used_micros),
            warning_percent: budget.warning_percent,
            usage_percent,
            unpriced_requests,
            period_start_ms: budget.period_start_ms,
            period_end_ms: budget.period_end_ms,
            expiry_warning_days: budget.expiry_warning_days,
            usage_level,
            expiry_level,
            enforcement: BudgetEnforcement::ObserveOnly,
            routing_affected: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> AgentBudget {
        AgentBudget {
            limit_micros: 1_000_000,
            warning_percent: 80,
            period_start_ms: Some(1_000),
            period_end_ms: Some(10 * DAY_MS),
            expiry_warning_days: 7,
        }
    }

    #[test]
    fn usage_levels_are_informational_and_never_enforce_routing() {
        let healthy = BudgetStatus::evaluate("codex", &budget(), 700_000, 0, 2 * DAY_MS);
        let approaching = BudgetStatus::evaluate("codex", &budget(), 800_000, 0, 2 * DAY_MS);
        let exceeded = BudgetStatus::evaluate("codex", &budget(), 1_100_000, 0, 2 * DAY_MS);
        assert_eq!(healthy.usage_level, BudgetUsageLevel::Healthy);
        assert_eq!(approaching.usage_level, BudgetUsageLevel::Approaching);
        assert_eq!(exceeded.usage_level, BudgetUsageLevel::Exceeded);
        for status in [healthy, approaching, exceeded] {
            assert_eq!(status.enforcement, BudgetEnforcement::ObserveOnly);
            assert!(!status.routing_affected);
        }
    }

    #[test]
    fn unknown_prices_and_expiry_are_reported_without_claiming_zero_spend() {
        let unknown = BudgetStatus::evaluate("claude-code", &budget(), 100, 2, 3 * DAY_MS);
        assert_eq!(unknown.usage_level, BudgetUsageLevel::Unknown);
        assert_eq!(unknown.expiry_level, BudgetExpiryLevel::Expiring);
        let expired = BudgetStatus::evaluate("claude-code", &budget(), 100, 0, 11 * DAY_MS);
        assert_eq!(expired.expiry_level, BudgetExpiryLevel::Expired);
    }

    #[test]
    fn invalid_thresholds_and_periods_are_rejected() {
        let mut value = budget();
        value.warning_percent = 0;
        assert!(value.validate().is_err());
        value.warning_percent = 80;
        value.period_start_ms = Some(10);
        value.period_end_ms = Some(9);
        assert!(value.validate().is_err());
    }
}
