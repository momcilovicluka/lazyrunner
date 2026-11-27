//! A source's databases (#114): what a server source's login may see there - each database's name, size, when it was
//! made and when it last changed - read only, from `master`, and kept in the state folder with when it was listed, so
//! opening the database screen never reaches the server. Whatever the login may not see is `None` (`-` on the
//! screen); a missing permission never fails the list: the sizes come from `sys.master_files` where it shows them,
//! the changes from `sys.dm_db_index_usage_stats` (since the server's last start) only where `VIEW SERVER STATE` is
//! granted - that query's failure is ignored.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::sql::{self, Sql};

/// The project's state folder keeps each source's last list.
pub const FILE: &str = "catalogs.json";
/// The databases, their sizes and when each was made: the server's own (ids 1-4) and snapshots left out.
pub const LIST_SQL: &str = "SET NOCOUNT ON; \
     SELECT ISNULL(CONVERT(varchar(30), (SELECT SUM(CAST(f.size AS bigint)) * 8192 FROM sys.master_files f \
     WHERE f.database_id = d.database_id)), '-'), \
     CONVERT(varchar(40), TODATETIMEOFFSET(d.create_date, DATEPART(TZOFFSET, SYSDATETIMEOFFSET())), 127), d.name \
     FROM sys.databases d WHERE d.database_id > 4 AND d.source_database_id IS NULL ORDER BY d.name;";
/// When each database last changed, as far as the server knows since it started.
pub const CHANGED_SQL: &str = "SET NOCOUNT ON; \
     SELECT CONVERT(varchar(40), TODATETIMEOFFSET(MAX(s.last_user_update), DATEPART(TZOFFSET, SYSDATETIMEOFFSET())), \
     127), DB_NAME(s.database_id) FROM sys.dm_db_index_usage_stats s WHERE s.database_id > 4 \
     GROUP BY s.database_id;";
/// What a value the login may not see reads as.
const UNSEEN: &str = "-";

/// A database on a source's server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<String>,
}

/// A source's last list: when (RFC 3339), and its databases.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub at: String,
    pub databases: Vec<Listed>,
}

impl Catalog {
    pub fn database(&self, name: &str) -> Option<&Listed> {
        self.databases.iter().find(|listed| listed.name.eq_ignore_ascii_case(name))
    }
}

fn seen(value: &str) -> Option<String> {
    Some(value.trim().to_string()).filter(|value| !value.is_empty() && value != UNSEEN && value != "NULL")
}

/// The databases a listing's rows name, with when each changed from the second query's rows.
pub fn read(listed: &[Vec<String>], changed: &[Vec<String>]) -> Vec<Listed> {
    let changed: BTreeMap<String, String> =
        changed.iter().filter_map(|row| Some((row.get(1)?.to_lowercase(), seen(row.first()?)?))).collect();
    listed
        .iter()
        .filter_map(|row| {
            let [bytes, created, name] = row.as_slice() else { return None };
            Some(Listed {
                name: name.clone(),
                bytes: seen(bytes).and_then(|bytes| bytes.parse().ok()),
                created: seen(created),
                changed: changed.get(&name.to_lowercase()).cloned(),
            })
        })
        .collect()
}

/// Lists a server's databases through `connection` (run in `master`); only the list itself failing is an error.
pub fn list(sql: &dyn Sql, connection: &str) -> Result<Vec<Listed>, String> {
    let master = Some(super::pull::MASTER);
    let listed = sql::query(sql, connection, master, LIST_SQL, 3)?;
    let changed = sql::query(sql, connection, master, CHANGED_SQL, 2).unwrap_or_default();
    Ok(read(&listed, &changed))
}

/// Each source's last list; none when the file is missing or broken.
pub fn load(state_dir: &Path) -> BTreeMap<String, Catalog> {
    match crate::json::read(&state_dir.join(FILE)) {
        Some(Value::Object(kept)) => kept
            .into_iter()
            .filter_map(|(source, catalog)| Some((source, serde_json::from_value(catalog).ok()?)))
            .collect(),
        _ => BTreeMap::new(),
    }
}

pub fn save(state_dir: &Path, all: &BTreeMap<String, Catalog>) -> Result<()> {
    crate::config::write(&state_dir.join(FILE), &serde_json::to_value(all)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|cell| cell.to_string()).collect()
    }

    #[test]
    fn a_listing_reads_what_the_login_may_see_and_dashes_the_rest() {
        let listed = [
            row(&["4509715660", "2026-01-12T09:00:00.123+01:00", "Shop"]),
            row(&["-", "2026-03-02T10:00:00+01:00", "Shop_Audit"]),
            row(&["NULL", "", "Tenant_Acme"]),
        ];
        let changed = [row(&["2026-11-27T08:15:00+01:00", "SHOP"]), row(&["NULL", "Shop_Audit"])];
        let read = read(&listed, &changed);
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].bytes, Some(4_509_715_660));
        assert_eq!(read[0].changed.as_deref(), Some("2026-11-27T08:15:00+01:00"), "matched ignoring case");
        assert_eq!((read[1].bytes, read[1].changed.as_ref()), (None, None));
        assert_eq!((read[2].created.as_ref(), read[2].bytes), (None, None));
    }

    /// SQL whose second query fails, as it does for a login without VIEW SERVER STATE.
    struct NoServerState;

    impl Sql for NoServerState {
        fn run(&self, _: &str, database: Option<&str>, text: &str, line: &mut dyn FnMut(&str)) -> Result<(), String> {
            assert_eq!(database, Some("master"));
            if text == CHANGED_SQL {
                return Err("VIEW SERVER STATE permission was denied".into());
            }
            line(&["120", "2026-01-12T09:00:00+01:00", "Shop"].join(sql::SEPARATOR));
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn a_missing_permission_never_fails_the_list_and_the_list_is_kept() {
        let listed = list(&NoServerState, "Server=qa").unwrap();
        assert_eq!(
            listed,
            [Listed { name: "Shop".into(), bytes: Some(120), created: listed[0].created.clone(), changed: None }]
        );
        let temp = crate::testing::TempDir::new("catalogs");
        let mut all = BTreeMap::new();
        all.insert("qa".to_string(), Catalog { at: "2026-11-27T09:00:00+01:00".into(), databases: listed });
        save(temp.path(), &all).unwrap();
        assert_eq!(load(temp.path()), all);
        assert_eq!(all["qa"].database("shop").map(|found| found.bytes), Some(Some(120)));
    }
}
