// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal, dependency-free argument parsing.
//!
//! Global flags may appear anywhere. Per-command flags are read by [`Args::flag`],
//! [`Args::value`] and [`Args::values`]. Unknown flags are rejected by each command.

use tpt_lean_core::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct Args {
    pub json: bool,
    pub ndjson: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub project: Option<String>,
    pub overrides: Vec<(String, String)>,
    /// `--help` or `-h` appeared anywhere on the command line.
    pub help: bool,
    /// Subcommand path, e.g. `["index", "scan"]`.
    pub command: Vec<String>,
    /// Remaining positionals and per-command flags, in order.
    pub rest: Vec<String>,
}

impl Args {
    pub fn parse(argv: &[String]) -> Result<Self> {
        let mut out = Args::default();
        let mut i = 0;
        let mut after_double_dash = false;
        while i < argv.len() {
            let a = argv[i].as_str();
            if after_double_dash {
                out.rest.push(a.to_string());
            } else if a == "--" {
                after_double_dash = true;
            } else if a == "--json" {
                out.json = true;
            } else if a == "--ndjson" {
                out.ndjson = true;
            } else if a == "--verbose" || a == "-v" {
                out.verbose = true;
            } else if a == "--quiet" || a == "-q" {
                out.quiet = true;
            } else if a == "--project" {
                i += 1;
                out.project = Some(
                    argv.get(i)
                        .cloned()
                        .ok_or_else(|| Error::invalid("--project needs a value"))?,
                );
            } else if let Some(v) = a.strip_prefix("--project=") {
                out.project = Some(v.to_string());
            } else if a == "--set" {
                i += 1;
                let kv = argv
                    .get(i)
                    .ok_or_else(|| Error::invalid("--set needs section.key=value"))?;
                out.overrides.push(split_kv(kv)?);
            } else if a == "--help" || a == "-h" {
                out.help = true;
            } else {
                out.rest.push(a.to_string());
            }
            i += 1;
        }
        // Subcommand path: take leading words that are known two-level commands, else one word.
        let two_level = ["index", "cache", "server", "mcp", "config", "completions"];
        if let Some(first) = out.rest.first().cloned() {
            if two_level.contains(&first.as_str()) && out.rest.len() > 1 && !out.rest[1].starts_with('-') {
                out.command = vec![first, out.rest[1].clone()];
                out.rest.drain(0..2);
            } else {
                out.command = vec![first];
                out.rest.remove(0);
            }
        }
        Ok(out)
    }

    /// True if `--name` appears among the per-command arguments.
    pub fn flag(&self, name: &str) -> bool {
        self.rest.iter().any(|a| a == name)
    }

    /// Value of `--name value` or `--name=value` (first occurrence).
    pub fn value(&self, name: &str) -> Result<Option<String>> {
        let prefix = format!("{name}=");
        for (i, a) in self.rest.iter().enumerate() {
            if a == name {
                return self
                    .rest
                    .get(i + 1)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| Error::invalid(format!("{name} needs a value")));
            }
            if let Some(v) = a.strip_prefix(&prefix) {
                return Ok(Some(v.to_string()));
            }
        }
        Ok(None)
    }

    /// Every value given for a repeatable flag.
    pub fn values(&self, name: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.rest.len() {
            if self.rest[i] == name {
                if let Some(v) = self.rest.get(i + 1) {
                    out.push(v.clone());
                }
                i += 2;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Positional arguments: everything that is not a flag or a flag value.
    pub fn positionals(&self, value_flags: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.rest.len() {
            let a = &self.rest[i];
            if value_flags.contains(&a.as_str()) {
                i += 2;
                continue;
            }
            if a.starts_with("--") {
                i += 1;
                continue;
            }
            out.push(a.clone());
            i += 1;
        }
        out
    }

    /// Reject unknown `--flags` for a command. `known` lists every flag the command accepts.
    pub fn reject_unknown(&self, known: &[&str], value_flags: &[&str]) -> Result<()> {
        let mut i = 0;
        while i < self.rest.len() {
            let a = &self.rest[i];
            if a.starts_with("--") && !a.contains('=') {
                if !known.contains(&a.as_str()) {
                    return Err(Error::invalid(format!("unknown flag {a}")));
                }
                i += if value_flags.contains(&a.as_str()) { 2 } else { 1 };
                continue;
            }
            if a.starts_with("--") {
                let name = a.split('=').next().unwrap_or(a);
                if !known.contains(&name) {
                    return Err(Error::invalid(format!("unknown flag {name}")));
                }
            }
            i += 1;
        }
        Ok(())
    }
}

fn split_kv(kv: &str) -> Result<(String, String)> {
    let (k, v) = kv
        .split_once('=')
        .ok_or_else(|| Error::invalid(format!("expected key=value, got {kv:?}")))?;
    if !k.contains('.') {
        return Err(Error::invalid(format!("config key must be section.key: {k}")));
    }
    Ok((k.to_string(), v.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn global_flags_anywhere_and_subcommands() {
        let a = Args::parse(&v(&["index", "--json", "scan", "--project", "p"])).unwrap();
        assert!(a.json);
        assert_eq!(a.command, vec!["index".to_string(), "scan".to_string()]);
        assert_eq!(a.project.as_deref(), Some("p"));
    }

    #[test]
    fn set_overrides_parse() {
        let a = Args::parse(&v(&["doctor", "--set", "build.cache=false"])).unwrap();
        assert_eq!(
            a.overrides,
            vec![("build.cache".to_string(), "false".to_string())]
        );
        assert!(Args::parse(&v(&["doctor", "--set", "nodot=1"])).is_err());
    }

    #[test]
    fn value_and_values() {
        let a = Args::parse(&v(&["search", "x", "--near", "a", "--near=b", "--limit", "5"])).unwrap();
        assert_eq!(a.value("--limit").unwrap().as_deref(), Some("5"));
        assert_eq!(a.values("--near"), vec!["a".to_string()]);
        assert_eq!(a.positionals(&["--limit", "--near"]), vec!["x".to_string()]);
    }

    #[test]
    fn unknown_flags_rejected() {
        let a = Args::parse(&v(&["fmt", "--bogus"])).unwrap();
        assert!(a.reject_unknown(&["--check"], &[]).is_err());
        let b = Args::parse(&v(&["fmt", "--check"])).unwrap();
        assert!(b.reject_unknown(&["--check"], &[]).is_ok());
    }
}
