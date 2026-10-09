use rusqlite::Connection;
use serde_json::{Map, Value, json};

use crate::StoreError;

/// Ordered SQLite migrations; the PostgreSQL equivalents live in
/// `migrations/postgres/` with the same numbers.
pub(crate) const MIGRATIONS: [&str; 3] = [
    include_str!("../migrations/sqlite/0001_projection.sql"),
    include_str!("../migrations/sqlite/0002_worktrees.sql"),
    include_str!("../migrations/sqlite/0003_runs.sql"),
];

/// Identifier of the schema description format (`migrations/schema.v0.json`).
pub(crate) const SCHEMA_DESCRIPTION: &str = "libre-ai.work-supervision.projection.v0";

/// Applies every migration newer than the database's `user_version`, each in its own transaction.
pub(crate) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let current: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let current = usize::try_from(current).map_err(|_| StoreError::Sqlite)?;
    if current > MIGRATIONS.len() {
        return Err(StoreError::SchemaTooNew);
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let transaction = connection.transaction()?;
        transaction.execute_batch(sql)?;
        transaction.execute_batch(&format!("PRAGMA user_version = {}", index + 1))?;
        transaction.commit()?;
    }
    Ok(())
}

/// Tables of the projection, sorted by name.
pub(crate) fn tables(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

/// Columns of `table` in declaration order: name, declared type, not-null flag, primary-key rank.
pub(crate) fn columns(
    connection: &Connection,
    table: &str,
) -> Result<Vec<(String, String, bool, i64)>, StoreError> {
    let mut statement =
        connection.prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1)")?;
    let columns = statement
        .query_map([table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}

/// Describes the schema in the format of `migrations/schema.v0.json`.
pub(crate) fn describe(connection: &Connection) -> Result<Value, StoreError> {
    let mut described = Vec::new();
    for table in tables(connection)? {
        let columns = columns(connection, &table)?;
        let mut keys: Vec<(i64, String)> = columns
            .iter()
            .filter(|(_, _, _, rank)| *rank > 0)
            .map(|(name, _, _, rank)| (*rank, name.clone()))
            .collect();
        keys.sort();
        let mut entry = Map::new();
        entry.insert("name".to_owned(), Value::String(table));
        entry.insert(
            "columns".to_owned(),
            Value::Array(
                columns
                    .iter()
                    .map(|(name, declared, not_null, _)| {
                        json!({
                            "name": name,
                            "type": declared.to_ascii_lowercase(),
                            "nullable": !not_null,
                        })
                    })
                    .collect(),
            ),
        );
        entry.insert(
            "primary_key".to_owned(),
            Value::Array(
                keys.into_iter()
                    .map(|(_, name)| Value::String(name))
                    .collect(),
            ),
        );
        described.push(Value::Object(entry));
    }
    Ok(json!({ "schema": SCHEMA_DESCRIPTION, "tables": described }))
}
