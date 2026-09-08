use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    #[default]
    Free,
    Starter,
    Pro,
    Enterprise,
}

impl Plan {
    pub fn as_str(&self) -> &'static str {
        match self {
            Plan::Free => "free",
            Plan::Starter => "starter",
            Plan::Pro => "pro",
            Plan::Enterprise => "enterprise",
        }
    }

    pub fn from_str(s: &str) -> Plan {
        match s.to_lowercase().as_str() {
            "starter" => Plan::Starter,
            "pro" => Plan::Pro,
            "enterprise" => Plan::Enterprise,
            _ => Plan::Free,
        }
    }

    #[allow(dead_code)]
    pub fn price_monthly(&self) -> u64 {
        match self {
            Plan::Free => 0,
            Plan::Starter => 9,
            Plan::Pro => 29,
            Plan::Enterprise => 99,
        }
    }

    pub fn max_databases(&self) -> usize {
        match self {
            Plan::Free => 20,
            Plan::Starter => 50,
            Plan::Pro => 200,
            Plan::Enterprise => 1000,
        }
    }

    pub fn max_queries_per_minute(&self) -> u64 {
        match self {
            Plan::Free => 60,
            Plan::Starter => 300,
            Plan::Pro => 2000,
            Plan::Enterprise => 10000,
        }
    }

    #[allow(dead_code)]
    pub fn storage_gb(&self) -> u64 {
        match self {
            Plan::Free => 1,
            Plan::Starter => 5,
            Plan::Pro => 20,
            Plan::Enterprise => 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_plans_case_insensitively() {
        assert_eq!(Plan::from_str("starter"), Plan::Starter);
        assert_eq!(Plan::from_str("STARTER"), Plan::Starter);
        assert_eq!(Plan::from_str("Pro"), Plan::Pro);
        assert_eq!(Plan::from_str("ENTERPRISE"), Plan::Enterprise);
    }

    #[test]
    fn unknown_plan_defaults_to_free() {
        assert_eq!(Plan::from_str("hacker-level"), Plan::Free);
        assert_eq!(Plan::from_str(""), Plan::Free);
        assert_eq!(Plan::from_str("  "), Plan::Free);
    }

    #[test]
    fn plan_limits_are_ordered() {
        assert!(Plan::Free.max_databases() < Plan::Starter.max_databases());
        assert!(Plan::Starter.max_databases() < Plan::Pro.max_databases());
        assert!(Plan::Pro.max_databases() < Plan::Enterprise.max_databases());

        assert!(Plan::Free.max_queries_per_minute() < Plan::Starter.max_queries_per_minute());
        assert!(Plan::Starter.max_queries_per_minute() < Plan::Pro.max_queries_per_minute());
        assert!(Plan::Pro.max_queries_per_minute() < Plan::Enterprise.max_queries_per_minute());
    }

    #[test]
    fn as_str_roundtrips_through_from_str() {
        for plan in [Plan::Free, Plan::Starter, Plan::Pro, Plan::Enterprise] {
            assert_eq!(Plan::from_str(plan.as_str()), plan);
        }
    }

    #[test]
    fn free_plan_is_default() {
        assert_eq!(Plan::default(), Plan::Free);
    }
}
