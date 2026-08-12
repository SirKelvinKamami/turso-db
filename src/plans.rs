use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
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

    pub fn storage_gb(&self) -> u64 {
        match self {
            Plan::Free => 1,
            Plan::Starter => 5,
            Plan::Pro => 20,
            Plan::Enterprise => 100,
        }
    }
}

impl Default for Plan {
    fn default() -> Self {
        Plan::Free
    }
}
