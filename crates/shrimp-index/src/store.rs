use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::{FileMeta, IndexError, Symbol};

const FILES: TableDefinition<&str, &str> = TableDefinition::new("files");
const SYMBOLS: TableDefinition<&str, &str> = TableDefinition::new("symbols");

pub struct IndexStore {
    db: Database,
}

fn needs_db_rebuild(msg: &str) -> bool {
    msg.contains("Manual upgrade")
        || msg.contains("format version")
        || msg.contains("not a redb database")
        || msg.contains("not a valid redb")
}

fn init_tables(db: Database) -> Result<IndexStore, IndexError> {
    let txn = db.begin_write()?;
    {
        let _f = txn.open_table(FILES)?;
        let _s = txn.open_table(SYMBOLS)?;
    }
    txn.commit()?;
    Ok(IndexStore { db })
}

fn try_open(path: &Path) -> Result<IndexStore, IndexError> {
    init_tables(Database::create(path)?)
}

fn rebuild_db(path: &Path) -> Result<IndexStore, IndexError> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    try_open(path)
}

impl IndexStore {
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        match try_open(path) {
            Ok(store) => Ok(store),
            Err(IndexError::Db(msg)) if needs_db_rebuild(&msg) => rebuild_db(path),
            Err(e) => Err(e),
        }
    }

    pub fn get_file_meta(&self, file: &str) -> Result<Option<FileMeta>, IndexError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(FILES)?;
        match table.get(file)? {
            Some(v) => {
                let meta = serde_json::from_str(v.value())
                    .map_err(|e| IndexError::Parse(e.to_string()))?;
                Ok(Some(meta))
            }
            None => Ok(None),
        }
    }

    pub fn upsert_file(&self, meta: &FileMeta, symbols: &[Symbol]) -> Result<(), IndexError> {
        let write_txn = self.db.begin_write()?;
        let prefix = format!("{}::", meta.file);
        {
            let mut sym_table = write_txn.open_table(SYMBOLS)?;
            let to_delete: Vec<String> = {
                let mut keys = Vec::new();
                for entry in sym_table.range(prefix.as_str()..)? {
                    let (k, _) = entry?;
                    if !k.value().starts_with(&prefix) {
                        break;
                    }
                    keys.push(k.value().to_owned());
                }
                keys
            };
            for key in &to_delete {
                sym_table.remove(key.as_str())?;
            }
            for sym in symbols {
                let key = format!("{}::{}::{}", sym.file, sym.kind.as_str(), sym.name);
                let val =
                    serde_json::to_string(sym).map_err(|e| IndexError::Parse(e.to_string()))?;
                sym_table.insert(key.as_str(), val.as_str())?;
            }
        }
        {
            let mut file_table = write_txn.open_table(FILES)?;
            let val = serde_json::to_string(meta).map_err(|e| IndexError::Parse(e.to_string()))?;
            file_table.insert(meta.file.as_str(), val.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn remove_file(&self, file: &str) -> Result<(), IndexError> {
        let write_txn = self.db.begin_write()?;
        let prefix = format!("{file}::");
        {
            let mut sym_table = write_txn.open_table(SYMBOLS)?;
            let to_delete: Vec<String> = {
                let mut keys = Vec::new();
                for entry in sym_table.range(prefix.as_str()..)? {
                    let (k, _) = entry?;
                    if !k.value().starts_with(&prefix) {
                        break;
                    }
                    keys.push(k.value().to_owned());
                }
                keys
            };
            for key in &to_delete {
                sym_table.remove(key.as_str())?;
            }
        }
        {
            let mut file_table = write_txn.open_table(FILES)?;
            file_table.remove(file)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn all_files(&self) -> Result<Vec<FileMeta>, IndexError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(FILES)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let meta: FileMeta =
                serde_json::from_str(v.value()).map_err(|e| IndexError::Parse(e.to_string()))?;
            out.push(meta);
        }
        Ok(out)
    }

    pub fn all_symbols(&self) -> Result<Vec<Symbol>, IndexError> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SYMBOLS)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let sym: Symbol =
                serde_json::from_str(v.value()).map_err(|e| IndexError::Parse(e.to_string()))?;
            out.push(sym);
        }
        Ok(out)
    }

    pub fn lookup_symbol(&self, query: &str) -> Result<Vec<Symbol>, IndexError> {
        let ql = query.to_lowercase();
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SYMBOLS)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let sym: Symbol =
                serde_json::from_str(v.value()).map_err(|e| IndexError::Parse(e.to_string()))?;
            if sym.name.to_lowercase().contains(&ql) {
                out.push(sym);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_stale_redb_format() {
        let msg = "Manual upgrade required. Expected file format version 3, but file is version 2";
        assert!(needs_db_rebuild(msg));
    }
}
