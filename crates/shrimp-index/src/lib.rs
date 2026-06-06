pub mod build;
pub mod parser;
pub mod store;

pub use build::build_index;
pub use store::IndexStore;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Struct,
    Trait,
    Enum,
    Impl,
    Module,
    Class,
    Method,
    Variable,
    Other,
}

impl SymbolKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Trait => "trait",
            Self::Enum => "enum",
            Self::Impl => "impl",
            Self::Module => "module",
            Self::Class => "class",
            Self::Method => "method",
            Self::Variable => "variable",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file: String,
    pub line: u32,
    pub end_line: u32,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub file: String,
    pub hash: String,
    pub mtime: u64,
    pub symbol_count: usize,
}

#[derive(Debug, Default)]
pub struct IndexStats {
    pub files_total: usize,
    pub files_changed: usize,
    pub files_removed: usize,
    pub symbols_total: usize,
    pub duration_ms: u64,
}

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("database: {0}")]
    Db(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("walk: {0}")]
    Walk(String),
}

impl From<redb::Error> for IndexError {
    fn from(e: redb::Error) -> Self {
        Self::Db(e.to_string())
    }
}

impl From<redb::DatabaseError> for IndexError {
    fn from(e: redb::DatabaseError) -> Self {
        Self::Db(e.to_string())
    }
}

impl From<redb::TransactionError> for IndexError {
    fn from(e: redb::TransactionError) -> Self {
        Self::Db(e.to_string())
    }
}

impl From<redb::TableError> for IndexError {
    fn from(e: redb::TableError) -> Self {
        Self::Db(e.to_string())
    }
}

impl From<redb::StorageError> for IndexError {
    fn from(e: redb::StorageError) -> Self {
        Self::Db(e.to_string())
    }
}

impl From<redb::CommitError> for IndexError {
    fn from(e: redb::CommitError) -> Self {
        Self::Db(e.to_string())
    }
}

impl From<ignore::Error> for IndexError {
    fn from(e: ignore::Error) -> Self {
        Self::Walk(e.to_string())
    }
}
