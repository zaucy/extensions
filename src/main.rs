use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use derive_more::Deref;
use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tempfile::TempDir;
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<()> {
    let should_publish = false;

    let extensions_toml: ExtensionsToml = read_toml_file("extensions.toml").await?;

    fs::create_dir_all("build").await?;

    let extension_ids = if should_publish {
        unpublished_extension_ids(&extensions_toml).await?
    } else {
        changed_extension_ids(&extensions_toml).await?
    };

    for extension_id in extension_ids {
        let Some(extension_info) = extensions_toml.get(&extension_id) else {
            println!("No extension info found for '{extension_id}'.");
            continue;
        };

        println!(
            "Packaging '{extension_id}'. Version: {}",
            extension_info.version
        );

        package_extension(
            extension_id,
            &extension_info.path,
            &extension_info.version,
            should_publish,
        )
        .await?;
    }

    fs::remove_dir_all("build").await?;

    Ok(())
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Deserialize)]
struct ExtensionId(String);

impl fmt::Display for ExtensionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Deserialize)]
struct ExtensionInfo {
    pub path: PathBuf,
    pub version: String,
}

#[derive(Debug, Deref, Deserialize)]
struct ExtensionsToml(IndexMap<ExtensionId, ExtensionInfo>);

async fn package_extension(
    extension_id: ExtensionId,
    extension_path: &Path,
    extension_version: &str,
    should_publish: bool,
) -> Result<()> {
    Ok(())
}

async fn read_extension_manifest(extension_path: &str) {}

async fn read_json_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T> {
    let mut json_file = File::open(path).await?;

    let mut buffer = String::new();
    json_file.read_to_string(&mut buffer).await?;

    Ok(serde_json_lenient::from_str(&buffer)?)
}

async fn read_toml_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T> {
    let mut toml_file = File::open(path).await?;

    let mut buffer = String::new();
    toml_file.read_to_string(&mut buffer).await?;

    Ok(toml::from_str(&buffer)?)
}

async fn get_published_versions_by_extension_id() -> Result<HashMap<ExtensionId, Vec<String>>> {
    Ok(HashMap::new())
}

/// Returns the list of IDs of extensions that need to be published.
async fn unpublished_extension_ids(extensions_toml: &ExtensionsToml) -> Result<Vec<ExtensionId>> {
    let published_extension_versions = get_published_versions_by_extension_id().await?;

    let mut unpublished = Vec::new();
    for (extension_id, extension_info) in extensions_toml.iter() {
        let Some(versions) = published_extension_versions.get(&extension_id) else {
            continue;
        };

        if versions.contains(&extension_info.version) {
            unpublished.push(extension_id.clone());
        }
    }

    println!(
        "Extensions needing to be published: {}",
        unpublished
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(unpublished)
}

async fn changed_extension_ids(extensions_toml: &ExtensionsToml) -> Result<Vec<ExtensionId>> {
    let output = Command::new("git")
        .args(&["show", "origin/main:extensions.toml"])
        .output()
        .await?;
    let main_extensions_toml: ExtensionsToml = toml::from_str(&String::from_utf8(output.stdout)?)?;

    let mut changed = Vec::new();
    for (extension_id, extension_info) in extensions_toml.iter() {
        let version_on_main = main_extensions_toml
            .get(extension_id)
            .map(|extension_info| extension_info.version.as_str());

        if version_on_main == Some(&extension_info.version) {
            continue;
        }

        changed.push(extension_id.clone());
    }

    println!(
        "Extensions changed from main: {}",
        changed
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(changed)
}

async fn checkout_git_repo(name: &str, repository_url: &str, commit_sha: &str) -> Result<TempDir> {
    let repo_dir = tempfile::tempdir_in("build")?;

    Command::new("git").arg("init").output().await?;
    Command::new("git")
        .args(&["remote", "add", "origin", repository_url])
        .output()
        .await?;
    Command::new("git")
        .args(&["fetch", "--depth", "1", "origin", commit_sha])
        .output()
        .await?;
    Command::new("git")
        .args(&["checkout", commit_sha])
        .output()
        .await?;

    Ok(repo_dir)
}
