use anyhow::Result;
use chrono::TimeZone;
use std::env;
use std::fs;
use std::process::Command;
use vergen_gitcl::{Emitter, GitclBuilder};

fn main() -> Result<()> {
    // Generate git information
    let gitcl = GitclBuilder::default()
        .describe(true, true, Some("[0-9]*"))
        .build()?;

    let gitcl_res = Emitter::default()
        .idempotent()
        .fail_on_error()
        .add_instructions(&gitcl)
        .and_then(|emitter| emitter.emit());

    if let Err(e) = gitcl_res {
        eprintln!("error occurred while generating instructions: {e:?}");
        Emitter::default().idempotent().fail_on_error().emit()?;
    }

    // Add build timestamp
    let now = match env::var("SOURCE_DATE_EPOCH") {
        Ok(val) => { chrono::Utc.timestamp_opt(val.parse::<i64>().unwrap(), 0).unwrap() }
        Err(_) => chrono::Utc::now(),
    };

    println!(
        "cargo:rustc-env=BUILD_TIMESTAMP={}",
        now.format("%Y-%m-%d %H:%M:%S UTC")
    );

    // Get truehd library version using cargo metadata
    let truehd_version = get_truehd_version_from_metadata().unwrap_or_else(|_| {
        read_truehd_version_fallback().unwrap_or_else(|_| "unknown".to_string())
    });
    println!("cargo:rustc-env=TRUEHD_VERSION={truehd_version}");

    // Tell cargo to rerun this build script if the truehd Cargo.toml changes
    println!("cargo:rerun-if-changed=truehd/Cargo.toml");

    Ok(())
}

/// Get truehd version using cargo metadata (works with published and local dependencies)
fn get_truehd_version_from_metadata() -> Result<String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("cargo metadata failed");
    }

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    // Method 1: Look for truehd in workspace members first (local development)
    if let Some(packages) = metadata["packages"].as_array() {
        for package in packages {
            if let Some(name) = package["name"].as_str() {
                if name == "truehd" {
                    if let Some(version) = package["version"].as_str() {
                        return Ok(version.to_string());
                    }
                }
            }
        }
    }

    // Method 2: Look in dependency graph for published truehd package
    if let Some(resolve) = metadata.get("resolve") {
        if let Some(nodes) = resolve["nodes"].as_array() {
            for node in nodes {
                if let Some(id) = node["id"].as_str() {
                    if id.starts_with("truehd ") {
                        // Extract version from "truehd 0.2.1 (registry+...)" format
                        if let Some(version_start) = id.find(' ') {
                            if let Some(version_end) = id[version_start + 1..].find(' ') {
                                let version =
                                    &id[version_start + 1..version_start + 1 + version_end];
                                return Ok(version.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!("truehd package not found in metadata");
}

/// Fallback: manually parse truehd/Cargo.toml (for edge cases)
fn read_truehd_version_fallback() -> Result<String> {
    let toml_content = fs::read_to_string("truehd/Cargo.toml")?;

    for line in toml_content.lines() {
        let line = line.trim();
        if line.starts_with("version") && line.contains("=") {
            if let Some(equals_pos) = line.find('=') {
                let version_part = line[equals_pos + 1..].trim();
                let version = version_part.trim_matches('"').trim_matches('\'');
                return Ok(version.to_string());
            }
        }
    }

    anyhow::bail!("Could not find version in truehd/Cargo.toml");
}
