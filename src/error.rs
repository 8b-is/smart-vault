use thiserror::Error;

/// Unified error type for all `smart-vault` operations.
#[derive(Debug, Error)]
pub enum MemVaultError {
    #[error("vault not found: {0}")]
    VaultNotFound(String),

    #[error("vault already initialized at {0}")]
    AlreadyInitialized(String),

    #[error("path escapes vault root or traverses a symlink: {0}")]
    SymlinkEscape(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("--apply requires --approved-plan-sha256 from the reviewed dry-run")]
    PlanApprovalRequired,

    #[error("the regenerated transaction differs from the reviewed plan; run a new dry-run")]
    PlanChanged,

    #[error("vault mutation lock is busy or invalid: {0}")]
    LockBusy(String),

    #[error("transaction error: {0}")]
    Transaction(String),

    #[error("invalid manifest or ledger: {0}")]
    InvalidManifest(String),

    #[error("invalid argument or validation failure: {0}")]
    Validation(String),

    #[error("unsafe or rejected URL: {0}")]
    UnsafeUrl(String),

    #[error("smart-tree (st) error: {0}")]
    SmartTree(String),

    #[error("memnet error: {0}")]
    MemNet(String),

    #[error("contract or package validation failed: {0}")]
    Contract(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl MemVaultError {
    /// Returns a stable uppercase error code suitable for CLI/MCP diagnostics.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::VaultNotFound(_) => "VAULT_NOT_FOUND",
            Self::AlreadyInitialized(_) => "ALREADY_INITIALIZED",
            Self::SymlinkEscape(_) => "SYMLINK_ESCAPE",
            Self::InvalidPath(_) => "INVALID_PATH",
            Self::PlanApprovalRequired => "PLAN_APPROVAL_REQUIRED",
            Self::PlanChanged => "PLAN_CHANGED",
            Self::LockBusy(_) => "LOCK_BUSY",
            Self::Transaction(_) => "TRANSACTION_ERROR",
            Self::InvalidManifest(_) => "INVALID_MANIFEST",
            Self::Validation(_) => "VALIDATION_ERROR",
            Self::UnsafeUrl(_) => "UNSAFE_URL",
            Self::SmartTree(_) => "SMART_TREE_ERROR",
            Self::MemNet(_) => "MEMNET_ERROR",
            Self::Contract(_) => "CONTRACT_ERROR",
            Self::Io(_) => "IO_ERROR",
            Self::Json(_) => "JSON_ERROR",
        }
    }
}

pub type Result<T> = std::result::Result<T, MemVaultError>;
