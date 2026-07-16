use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The app shell's persisted settings: one row (id=1) in the `settings` table
/// (db.rs SCHEMA_V6). Every rate stays in USD (CONTEXT.md Display Currency);
/// `currency`/`usd_rate` only govern how Cost is rendered, never what is stored.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct Settings {
    pub theme: String,    // "system" | "light" | "dark"
    pub language: String, // "en" | "zh-Hant"
    pub currency: String, // ISO 4217 code
    pub usd_rate: f64,    // 1 USD = usd_rate <currency>
    pub launch_at_login: bool,
    pub auto_check_updates: bool,
    pub first_run_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "system".to_string(),
            language: "en".to_string(),
            currency: "USD".to_string(),
            usd_rate: 1.0,
            launch_at_login: true,
            auto_check_updates: true,
            first_run_done: false,
        }
    }
}

/// Returns the stored settings, or defaults when the table is empty (fresh
/// install / never saved).
pub fn get_settings(conn: &Connection) -> rusqlite::Result<Settings> {
    conn.query_row(
        "SELECT theme, language, currency, usd_rate, launch_at_login, \
         auto_check_updates, first_run_done FROM settings WHERE id = 1",
        [],
        |r| {
            Ok(Settings {
                theme: r.get(0)?,
                language: r.get(1)?,
                currency: r.get(2)?,
                usd_rate: r.get(3)?,
                launch_at_login: r.get::<_, i64>(4)? != 0,
                auto_check_updates: r.get::<_, i64>(5)? != 0,
                first_run_done: r.get::<_, i64>(6)? != 0,
            })
        },
    )
    .optional()
    .map(|opt| opt.unwrap_or_default())
}

/// Whole-object upsert of the single settings row.
pub fn set_settings(conn: &Connection, s: &Settings) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings \
         (id, theme, language, currency, usd_rate, launch_at_login, auto_check_updates, first_run_done) \
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            s.theme,
            s.language,
            s.currency,
            s.usd_rate,
            s.launch_at_login as i64,
            s.auto_check_updates as i64,
            s.first_run_done as i64,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct UpdateStatus {
    pub state: String,          // "not-configured"
    pub version: Option<String>,
}

/// Honest stub: the real Tauri updater plugin is wired in a later wave. Until
/// then this reports not-configured rather than a fake "up to date".
pub fn check_updates() -> UpdateStatus {
    UpdateStatus {
        state: "not-configured".to_string(),
        version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_db(&dir.path().join("test.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn get_returns_defaults_when_unset() {
        let (_d, conn) = test_conn();
        let s = get_settings(&conn).unwrap();
        assert_eq!(s.theme, "system");
        assert_eq!(s.language, "en");
        assert_eq!(s.currency, "USD");
        assert_eq!(s.usd_rate, 1.0);
        assert!(s.launch_at_login);
        assert!(s.auto_check_updates);
        assert!(!s.first_run_done);
    }

    #[test]
    fn set_then_get_roundtrips() {
        let (_d, conn) = test_conn();
        let s = Settings {
            theme: "dark".to_string(),
            language: "zh-Hant".to_string(),
            currency: "HKD".to_string(),
            usd_rate: 7.8,
            launch_at_login: false,
            auto_check_updates: false,
            first_run_done: true,
        };
        set_settings(&conn, &s).unwrap();
        let got = get_settings(&conn).unwrap();
        assert_eq!(got.theme, "dark");
        assert_eq!(got.language, "zh-Hant");
        assert_eq!(got.currency, "HKD");
        assert_eq!(got.usd_rate, 7.8);
        assert!(!got.launch_at_login);
        assert!(!got.auto_check_updates);
        assert!(got.first_run_done);
    }

    #[test]
    fn set_is_whole_object_upsert() {
        let (_d, conn) = test_conn();
        set_settings(&conn, &Settings { theme: "light".to_string(), ..Default::default() }).unwrap();
        set_settings(&conn, &Settings { theme: "dark".to_string(), ..Default::default() }).unwrap();
        // Second write replaces the first; still exactly one row.
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(get_settings(&conn).unwrap().theme, "dark");
    }

    #[test]
    fn check_updates_reports_not_configured() {
        let u = check_updates();
        assert_eq!(u.state, "not-configured");
        assert_eq!(u.version, None);
    }
}
