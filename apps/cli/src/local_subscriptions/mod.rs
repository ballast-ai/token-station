//! Host-owned references to local subscription logins.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod claude;
pub mod copilot;
pub mod google;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Claude,
    Copilot,
    GeminiCli,
    Antigravity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub supports_images: Option<bool>,
    pub supports_tools: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub account: AccountView,
    pub base_url: String,
    pub models: Vec<Model>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

impl Kind {
    #[must_use]
    pub const fn provider(self) -> &'static str {
        match self {
            Self::Claude => "claude-plan",
            Self::Copilot => "copilot-plan",
            Self::GeminiCli => "gemini-cli-plan",
            Self::Antigravity => "antigravity-plan",
        }
    }
    #[must_use]
    pub const fn package(self) -> &'static str {
        match self {
            Self::Claude => "provider-claude-plan-v2",
            Self::Copilot => "provider-copilot-plan-v2",
            Self::GeminiCli | Self::Antigravity => "provider-codeassist-v2",
        }
    }
}

#[derive(Clone)]
pub enum AccountManager {
    Claude(claude::AccountManager),
    Copilot(copilot::AccountManager),
    Google(google::AccountManager),
}
macro_rules! dispatch {
    ($manager:expr, $method:ident $(, $argument:expr)*) => {
        match $manager {
            AccountManager::Claude(manager) => manager.$method($($argument),*),
            AccountManager::Copilot(manager) => manager.$method($($argument),*),
            AccountManager::Google(manager) => manager.$method($($argument),*),
        }
    };
}
impl AccountManager {
    #[must_use]
    pub fn new(data_dir: impl AsRef<std::path::Path>, kind: Kind) -> Self {
        match kind {
            Kind::Claude => Self::Claude(claude::AccountManager::new(data_dir)),
            Kind::Copilot => Self::Copilot(copilot::AccountManager::new(data_dir)),
            Kind::GeminiCli | Kind::Antigravity => {
                Self::Google(google::AccountManager::new(data_dir, kind))
            }
        }
    }
    /// # Errors
    /// The local login cannot be inspected safely.
    pub fn discover(&self) -> Result<AccountView, String> {
        dispatch!(self, discover)
    }
    /// # Errors
    /// The local reference store cannot be read.
    pub fn list(&self) -> Result<Vec<AccountView>, String> {
        dispatch!(self, list)
    }
    /// # Errors
    /// The local login cannot be bound to a reference.
    pub fn connect(&self) -> Result<AccountView, String> {
        dispatch!(self, connect)
    }
    /// # Errors
    /// The bound account cannot supply an observed model catalog.
    pub fn models(&self, id: &str) -> Result<Vec<Model>, String> {
        dispatch!(self, models, id)
    }
    /// # Errors
    /// The account or its transport metadata is unavailable.
    pub fn connection(&self, id: &str) -> Result<Connection, String> {
        dispatch!(self, connection, id)
    }
    /// # Errors
    /// The bound account changed or cannot supply an access credential.
    pub fn access_token(&self, id: &str) -> Result<String, String> {
        dispatch!(self, access_token, id)
    }
    /// # Errors
    /// The local reference cannot be removed.
    pub fn disconnect(&self, id: &str) -> Result<(), String> {
        dispatch!(self, disconnect, id)
    }
}

impl std::fmt::Debug for AccountManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LocalSubscriptionAccountManager")
    }
}
