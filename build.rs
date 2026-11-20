use anyhow::Result;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use vergen_gitcl::{Emitter, Gitcl};

fn main() -> Result<()> {
    // Generate git information
    let gitcl = Gitcl::builder()
        .describe(true, true, Some("[0-9]*"))
        .build();

    let gitcl_res = Emitter::default()
        .idempotent()
        .fail_on_error()
        .add_instructions(&gitcl)
        .and_then(|emitter| emitter.emit());

    if let Err(e) = gitcl_res {
        eprintln!("error occurred while generating instructions: {e:?}");

        // Building outside a git checkout, such as from a published crate:
        // describe the build by its package version instead.
        println!(
            "cargo:rustc-env=VERGEN_GIT_DESCRIBE={}",
            env::var("CARGO_PKG_VERSION")?
        );

        Emitter::default().idempotent().fail_on_error().emit()?;
    }

    // Add build timestamp
    let now = match env::var("SOURCE_DATE_EPOCH") {
        Ok(val) => val.parse()?,
        Err(_) => SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };

    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", format_utc(now));

    // Get truehd library version using cargo metadata
    let truehd_version = get_truehd_version_from_metadata().unwrap_or_else(|_| {
        read_truehd_version_fallback().unwrap_or_else(|_| "unknown".to_string())
    });
    println!("cargo:rustc-env=TRUEHD_VERSION={truehd_version}");

    // Tell cargo to rerun this build script if the truehd Cargo.toml changes
    println!("cargo:rerun-if-changed=truehd/Cargo.toml");

    // ...and when HEAD moves, or the version string keeps describing the commit the
    // binary was first built at. The ref file covers commits on a branch, HEAD covers
    // checkouts, and packed-refs covers a ref that only exists packed.
    let git_dir = Path::new(".git");
    if git_dir.exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
        println!("cargo:rerun-if-changed=.git/packed-refs");
        if let Ok(head) = fs::read_to_string(git_dir.join("HEAD"))
            && let Some(reference) = head.strip_prefix("ref: ")
        {
            println!("cargo:rerun-if-changed=.git/{}", reference.trim());
        }
    }

    Ok(())
}

/// Seconds since the Unix epoch as `YYYY-MM-DD HH:MM:SS UTC`.
fn format_utc(secs: u64) -> String {
    let (days, time) = (secs / 86_400, secs % 86_400);

    // Counted in years that start on 1 March, so a leap day is the last day of its year.
    let days = days + 719_468;
    let (era, day_of_era) = (days / 146_097, days % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month + 2) / 5 + 1;
    let (month, carry) = if month < 10 {
        (month + 3, 0)
    } else {
        (month - 9, 1)
    };
    let year = era * 400 + year_of_era + carry;

    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        time / 3_600,
        time % 3_600 / 60,
        time % 60
    )
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
            if package["name"].as_str() == Some("truehd")
                && let Some(version) = package["version"].as_str()
            {
                return Ok(version.to_string());
            }
        }
    }

    // Method 2: Look in dependency graph for published truehd package
    if let Some(resolve) = metadata.get("resolve")
        && let Some(nodes) = resolve["nodes"].as_array()
    {
        for node in nodes {
            if let Some(id) = node["id"].as_str()
                && id.starts_with("truehd ")
                // Extract version from "truehd 0.2.1 (registry+...)" format
                && let Some(version_start) = id.find(' ')
                && let Some(version_end) = id[version_start + 1..].find(' ')
            {
                let version = &id[version_start + 1..version_start + 1 + version_end];
                return Ok(version.to_string());
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
        if line.starts_with("version")
            && let Some(equals_pos) = line.find('=')
        {
            let version_part = line[equals_pos + 1..].trim();
            let version = version_part.trim_matches('"').trim_matches('\'');
            return Ok(version.to_string());
        }
    }

    anyhow::bail!("Could not find version in truehd/Cargo.toml");
}
