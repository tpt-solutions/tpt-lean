// SPDX-License-Identifier: MIT OR Apache-2.0
//! Configuration model and layered loading.
//!
//! Precedence (lowest to highest):
//! 1. built-in defaults
//! 2. project `tpt-lean.toml`
//! 3. `.tpt-lean/config.toml`
//! 4. environment variables (`TPT_LEAN_<SECTION>_<KEY>`)
//! 5. explicit overrides (CLI flags)

use crate::error::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use toml::{Table, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub project: ProjectConfig,
    pub build: BuildConfig,
    pub index: IndexConfig,
    pub server: ServerConfig,
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProjectConfig {
    pub name: String,
    pub lean_version: String,
    pub root: String,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            lean_version: "stable".into(),
            root: ".".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BuildConfig {
    /// `"auto"` or a positive integer as a string/number.
    pub parallelism: Parallelism,
    pub cache: bool,
    pub watch: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            parallelism: Parallelism::AUTO,
            cache: true,
            watch: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Parallelism {
    Auto(AutoTag),
    Fixed(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoTag {
    Auto,
}

impl Parallelism {
    pub const AUTO: Parallelism = Parallelism::Auto(AutoTag::Auto);

    /// Resolve to a concrete worker count.
    pub fn resolve(&self) -> usize {
        match self {
            Parallelism::Auto(_) => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            Parallelism::Fixed(n) => (*n).max(1),
        }
    }
}

impl Default for Parallelism {
    fn default() -> Self {
        Parallelism::AUTO
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IndexConfig {
    pub mathlib: bool,
    pub docstrings: bool,
    pub dependency_graph: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            mathlib: true,
            docstrings: true,
            dependency_graph: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    pub transport: String,
    pub allow_remote: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            transport: "uds".into(),
            allow_remote: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SecurityConfig {
    pub workspace_only: bool,
    pub allow_write: bool,
    pub allow_build: bool,
    pub command_timeout_seconds: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            workspace_only: true,
            allow_write: false,
            allow_build: true,
            command_timeout_seconds: 120,
        }
    }
}

/// Environment variables recognised as configuration overrides, mapped to dotted keys.
const ENV_KEYS: &[(&str, &str)] = &[
    ("TPT_LEAN_PROJECT_NAME", "project.name"),
    ("TPT_LEAN_PROJECT_LEAN_VERSION", "project.lean_version"),
    ("TPT_LEAN_BUILD_PARALLELISM", "build.parallelism"),
    ("TPT_LEAN_BUILD_CACHE", "build.cache"),
    ("TPT_LEAN_BUILD_WATCH", "build.watch"),
    ("TPT_LEAN_INDEX_MATHLIB", "index.mathlib"),
    ("TPT_LEAN_INDEX_DOCSTRINGS", "index.docstrings"),
    ("TPT_LEAN_INDEX_DEPENDENCY_GRAPH", "index.dependency_graph"),
    ("TPT_LEAN_SECURITY_ALLOW_WRITE", "security.allow_write"),
    ("TPT_LEAN_SECURITY_ALLOW_BUILD", "security.allow_build"),
    (
        "TPT_LEAN_SECURITY_COMMAND_TIMEOUT_SECONDS",
        "security.command_timeout_seconds",
    ),
];

/// Where each configuration value came from. Useful for `doctor` and debugging.
#[derive(Debug, Clone, Default)]
pub struct ConfigSources {
    pub files: Vec<String>,
    pub env: Vec<String>,
    pub overrides: Vec<String>,
}

/// Builder that assembles configuration from layers.
#[derive(Debug, Default)]
pub struct ConfigLoader {
    layers: Vec<Table>,
    sources: ConfigSources,
}

impl ConfigLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a TOML file layer. Missing files are ignored; malformed files are errors.
    pub fn with_file(mut self, path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(self);
        }
        let text = std::fs::read_to_string(path).map_err(|e| Error::from(e).with_path(path))?;
        let table: Table = toml::from_str(&text)
            .map_err(|e| Error::new(ErrorCode::ConfigInvalid, format!("{e}")).with_path(path))?;
        self.layers.push(table);
        self.sources.files.push(path.display().to_string());
        Ok(self)
    }

    /// Add environment layer from the given key/value pairs (usually `std::env::vars()`).
    pub fn with_env<I, K, V>(mut self, vars: I) -> Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let map: std::collections::HashMap<String, String> =
            vars.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        let mut table = Table::new();
        for (env_key, dotted) in ENV_KEYS {
            if let Some(raw) = map.get(*env_key) {
                let value = coerce(raw);
                insert_dotted(&mut table, dotted, value);
                self.sources.env.push((*env_key).to_string());
            }
        }
        self.layers.push(table);
        Ok(self)
    }

    /// Add explicit overrides (for example from CLI flags) given as `section.key` = raw text.
    pub fn with_overrides<'a, I>(mut self, overrides: I) -> Result<Self>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut table = Table::new();
        for (key, raw) in overrides {
            if !key.contains('.') {
                return Err(Error::invalid(format!("override key must be section.key: {key}")));
            }
            insert_dotted(&mut table, key, coerce(raw));
            self.sources.overrides.push(key.to_string());
        }
        self.layers.push(table);
        Ok(self)
    }

    /// Merge all layers over defaults and deserialize.
    pub fn build(self) -> Result<(Config, ConfigSources)> {
        let mut merged = Value::Table(Table::new());
        for layer in &self.layers {
            deep_merge(&mut merged, &Value::Table(layer.clone()));
        }
        let config: Config = merged
            .try_into()
            .map_err(|e: toml::de::Error| Error::new(ErrorCode::ConfigInvalid, e.to_string()))?;
        validate(&config)?;
        Ok((config, self.sources))
    }
}

/// Load configuration for a project root using the standard file layers and the process env.
pub fn load_config(project_root: &Path, overrides: &[(&str, &str)]) -> Result<(Config, ConfigSources)> {
    ConfigLoader::new()
        .with_file(&project_root.join("tpt-lean.toml"))?
        .with_file(&project_root.join(".tpt-lean").join("config.toml"))?
        .with_env(std::env::vars())?
        .with_overrides(overrides.iter().copied())?
        .build()
}

/// Reject values that are syntactically valid but unusable.
pub fn validate(config: &Config) -> Result<()> {
    if config.security.command_timeout_seconds == 0 {
        return Err(Error::new(
            ErrorCode::ConfigInvalid,
            "security.command_timeout_seconds must be > 0",
        ));
    }
    if let Parallelism::Fixed(0) = config.build.parallelism {
        return Err(Error::new(
            ErrorCode::ConfigInvalid,
            "build.parallelism must be \"auto\" or > 0",
        ));
    }
    if config.server.transport != "uds" && config.server.transport != "tcp" {
        return Err(Error::new(
            ErrorCode::ConfigInvalid,
            "server.transport must be \"uds\" or \"tcp\"",
        ));
    }
    Ok(())
}

/// Serialize the effective configuration as TOML (for `tpt-lean config show`).
pub fn to_toml(config: &Config) -> Result<String> {
    toml::to_string_pretty(config).map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))
}

fn coerce(raw: &str) -> Value {
    if let Ok(b) = raw.parse::<bool>() {
        return Value::Boolean(b);
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Value::Integer(n);
    }
    Value::String(raw.to_string())
}

fn insert_dotted(table: &mut Table, dotted: &str, value: Value) {
    let (section, key) = dotted.split_once('.').expect("dotted key checked by caller");
    let entry = table
        .entry(section.to_string())
        .or_insert_with(|| Value::Table(Table::new()));
    if let Value::Table(inner) = entry {
        inner.insert(key.to_string(), value);
    }
}

fn deep_merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Table(b), Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) => deep_merge(existing, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        let (cfg, _) = ConfigLoader::new().build().unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.security.command_timeout_seconds, 120);
        assert!(!cfg.server.allow_remote);
    }

    #[test]
    fn precedence_overrides_win_over_env_and_files() {
        let dir = std::env::temp_dir().join(format!("tpt-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tpt-lean.toml");
        std::fs::write(
            &file,
            "[build]\ncache = false\n[security]\ncommand_timeout_seconds = 30\n",
        )
        .unwrap();
        let (cfg, _) = ConfigLoader::new()
            .with_file(&file)
            .unwrap()
            .with_env([("TPT_LEAN_SECURITY_COMMAND_TIMEOUT_SECONDS", "60")])
            .unwrap()
            .with_overrides([("security.command_timeout_seconds", "90")])
            .unwrap()
            .build()
            .unwrap();
        assert!(!cfg.build.cache);
        assert_eq!(cfg.security.command_timeout_seconds, 90);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let dir = std::env::temp_dir().join(format!("tpt-cfg-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tpt-lean.toml");
        std::fs::write(&file, "[build]\nnope = 1\n").unwrap();
        let err = ConfigLoader::new().with_file(&file).unwrap().build().unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigInvalid);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_timeout_rejected() {
        let err = ConfigLoader::new()
            .with_overrides([("security.command_timeout_seconds", "0")])
            .unwrap()
            .build()
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    }

    #[test]
    fn parallelism_accepts_auto_and_numbers() {
        let (cfg, _) = ConfigLoader::new()
            .with_overrides([("build.parallelism", "4")])
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(cfg.build.parallelism.resolve(), 4);
        let (cfg, _) = ConfigLoader::new().build().unwrap();
        assert!(cfg.build.parallelism.resolve() >= 1);
    }
}
