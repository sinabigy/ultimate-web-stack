#![allow(clippy::unwrap_used)]
//! Every `APP__*` variable that the deployment tiers, docs and dev tooling set must name a real
//! configuration key. The loader rejects unknown keys at startup (deny_unknown_fields), so a typo
//! or a removed key in a systemd unit or Kubernetes manifest would crash-loop that service in
//! production; static manifest validation cannot see it. (This test exists because
//! `APP__WORKER__PORT` once had no config key and the worker unit failed live.)

use std::path::{Path, PathBuf};

use app_config::AppConfig;
use serde_json::Value;

const SCANNED: &[&str] = &["infra", "docs", ".env.example", "dev", "frontend/playwright.config.ts"];
/// Placeholders in prose, and map sections whose keys are user-defined names.
const PLACEHOLDERS: &[&str] = &["APP__SECTION__KEY"];
const MAPS: &[&str] = &["providers.definitions"];

fn files(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        for e in std::fs::read_dir(p).unwrap().flatten() {
            files(&e.path(), out);
        }
    } else if p.is_file() {
        out.push(p.to_path_buf());
    }
}

fn known(defaults: &Value, path: &[String]) -> bool {
    let mut v = defaults;
    for (i, k) in path.iter().enumerate() {
        let here = path[..i].join(".");
        if MAPS.contains(&here.as_str()) {
            return true;
        }
        match v.get(k) {
            Some(next) => v = next,
            None => return false,
        }
    }
    true
}

#[test]
fn every_app_variable_in_deployment_files_is_a_config_key() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let defaults = serde_json::to_value(AppConfig::default()).unwrap();
    let mut all = Vec::new();
    for s in SCANNED {
        files(&root.join(s), &mut all);
    }
    let mut unknown = Vec::new();
    let mut seen = 0;
    for f in all {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let mut rest = text.as_str();
        while let Some(i) = rest.find("APP__") {
            let tail = &rest[i..];
            let end =
                tail.find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')).unwrap_or(tail.len());
            let var = tail[..end].trim_end_matches('_');
            rest = &tail[end.max(5)..];
            if var.len() <= 5 || PLACEHOLDERS.contains(&var) {
                continue;
            }
            seen += 1;
            let path: Vec<String> = var["APP__".len()..].split("__").map(str::to_lowercase).collect();
            if !known(&defaults, &path) {
                unknown.push(format!("{var} in {}", f.strip_prefix(&root).unwrap_or(&f).display()));
            }
        }
    }
    assert!(seen > 30, "scanned too few variables ({seen}): wrong root?");
    assert!(unknown.is_empty(), "APP__ variables with no configuration key:\n{}", unknown.join("\n"));
}
