use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
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
    let (metadata, _format) = read_extension_manifest(&extension_path).await?;

    if metadata.version != extension_version {
        let error = [
            format!(
                "Incorrect version for extension {extension_id} ({name})",
                name = metadata.name
            ),
            "".to_string(),
            format!("Expected version: {extension_version}"),
            format!("Actual version: {}", metadata.version),
        ]
        .join("\n");

        bail!(error)
    }

    let mut package_manifest = ExtensionManifest {
        name: metadata.name.clone(),
        version: metadata.version,
        description: metadata.description,
        repository: metadata.repository,
        authors: metadata.authors,
        lib: None,
        themes: Vec::new(),
        languages: Vec::new(),
        grammars: IndexMap::new(),
        language_servers: IndexMap::new(),
    };

    let package_dir = tempfile::tempdir_in("build")?;
    let archive_name = package_dir
        .path()
        .join(format!("{extension_id}-{}", package_manifest.version))
        .with_extension("tar.gz");

    // let mut grammar_repo_paths = HashMap::new();

    let grammars_src_dir = extension_path.join("grammars");
    let languages_src_dir = extension_path.join("languages");
    let themes_src_dir = extension_path.join("themes");

    let grammars_pkg_dir = package_dir.path().join("grammars");
    let languages_pkg_dir = package_dir.path().join("languages");
    let themes_pkg_dir = package_dir.path().join("themes");

    if is_directory(&themes_src_dir).await {
        fs::create_dir(&themes_pkg_dir).await?;

        let mut read_dir = fs::read_dir(themes_src_dir).await?;
        while let Some(theme_entry) = read_dir.next_entry().await? {
            let Some(theme_filename) = theme_entry
                .file_name()
                .to_str()
                .map(|name| name.to_string())
            else {
                continue;
            };

            let theme: serde_json::Value = read_json_file(&theme_entry.path()).await?;

            validate_theme(&theme)?;

            let theme_destination_path = themes_pkg_dir.join(&theme_filename);
            fs::copy(theme_entry.path(), theme_destination_path).await?;
            package_manifest
                .themes
                .push(PathBuf::from_iter(["themes", &theme_filename]));
        }
    }

    Ok(())
}

async fn is_directory(path: impl AsRef<Path>) -> bool {
    match fs::metadata(path).await {
        Ok(metadata) => metadata.is_dir(),
        Err(_) => false,
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct ExtensionManifest {
    // pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub lib: Option<LibManifestEntry>,
    #[serde(default)]
    pub themes: Vec<PathBuf>,
    #[serde(default)]
    pub languages: Vec<PathBuf>,
    #[serde(default)]
    pub grammars: IndexMap<String, GrammarManifestEntry>,
    #[serde(default)]
    pub language_servers: IndexMap<String, LanguageServerManifestEntry>,
}

#[derive(Clone, Default, PartialEq, Eq, Debug, Deserialize)]
pub struct LibManifestEntry {
    path: String,
}

#[derive(Clone, Default, PartialEq, Eq, Debug, Deserialize)]
pub struct GrammarManifestEntry {
    repository: String,
    #[serde(alias = "commit")]
    rev: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize)]
pub struct LanguageServerManifestEntry {
    name: String,
    language: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
enum ExtensionManifestFormat {
    Toml,
    Json,
}

async fn read_extension_manifest(
    extension_path: &Path,
) -> Result<(ExtensionManifest, ExtensionManifestFormat)> {
    if let Some(manifest) =
        try_read_toml_file(extension_path.to_path_buf().join("extension.toml")).await?
    {
        return Ok((manifest, ExtensionManifestFormat::Toml));
    }

    let manifest = read_json_file(extension_path.to_path_buf().join("extension.json")).await?;
    Ok((manifest, ExtensionManifestFormat::Json))
}

async fn read_json_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T> {
    let mut json_file = File::open(path).await?;

    let mut buffer = String::new();
    json_file.read_to_string(&mut buffer).await?;

    Ok(serde_json_lenient::from_str(&buffer)?)
}

async fn try_read_toml_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<Option<T>> {
    let mut toml_file = match File::open(path).await {
        Ok(file) => file,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }

            Err(err)?
        }
    };

    let mut buffer = String::new();
    toml_file.read_to_string(&mut buffer).await?;

    Ok(toml::from_str(&buffer)?)
}

async fn read_toml_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T> {
    let mut toml_file = File::open(path).await?;

    let mut buffer = String::new();
    toml_file.read_to_string(&mut buffer).await?;

    Ok(toml::from_str(&buffer)?)
}

fn validate_theme(theme: &serde_json::Value) -> Result<()> {
    let json_schema: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/theme-family.json"))?;

    let mut scope = valico::json_schema::Scope::new();
    let schema = scope.compile_and_return(json_schema, false)?;

    let validation = schema.validate(&theme);
    if !validation.errors.is_empty() {
        bail!("Theme validation failed: {:?}", validation.errors);
    }

    Ok(())
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
