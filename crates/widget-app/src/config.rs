//! The widget's own tiny persisted config (`~/.claude/claude-widget.json`).
//!
//! Exists for exactly one thing today: the usage-API opt-in (issue #14). Consent must
//! be explicit and survive restarts/autostart, so it lives in a file the user toggles
//! via `claude-widget usage-api on|off` — never a default, never an env var.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Opt-in: poll the account usage endpoint with the stored OAuth token.
    pub usage_api: bool,
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".claude").join("claude-widget.json")
}

/// Load the config; a missing or unparseable file is the safe default (everything off).
pub fn load(path: &Path) -> Config {
    let Ok(text) = std::fs::read_to_string(path) else { return Config::default() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Config::default() };
    Config {
        usage_api: v.get("usage_api").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Persist the usage-API opt-in, preserving any other keys a future version may add.
pub fn set_usage_api(path: &Path, on: bool) -> std::io::Result<()> {
    let mut root = match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str::<Value>(&s).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    };
    if !root.is_object() {
        root = json!({});
    }
    root.as_object_mut().unwrap().insert("usage_api".into(), Value::Bool(on));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&root).unwrap_or_default() + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cw-cfg-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("claude-widget.json")
    }

    #[test]
    fn defaults_off_and_round_trips() {
        let p = temp("rt");
        assert!(!load(&p).usage_api, "missing file -> off");
        set_usage_api(&p, true).unwrap();
        assert!(load(&p).usage_api);
        set_usage_api(&p, false).unwrap();
        assert!(!load(&p).usage_api);
    }

    #[test]
    fn unparseable_config_is_off_and_foreign_keys_survive_toggle() {
        let p = temp("keys");
        std::fs::write(&p, "not json").unwrap();
        assert!(!load(&p).usage_api, "garbage -> safe default");
        std::fs::write(&p, r#"{"future_knob": 7}"#).unwrap();
        set_usage_api(&p, true).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["future_knob"], 7, "unrelated keys preserved");
        assert_eq!(v["usage_api"], true);
    }
}
