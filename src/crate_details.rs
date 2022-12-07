// Copyright 2019-2022 Parity Technologies (UK) Ltd.
// This file is part of subpub.
//
// subpub is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// subpub is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with subpub.  If not, see <http://www.gnu.org/licenses/>.

use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context};
use external::crates_io::CratesIoCrateVersion;
use semver::Version;
use strum::IntoEnumIterator;
use tracing::{info, span, Level};

use crate::{
    crates::{edit_all_dependency_sections, write_dependency_version, CrateDependencyKey},
    external::{self, cargo::PublishError},
    git::*,
    toml::{toml_read, toml_write},
    version::maybe_bump_for_breaking_change,
};

#[derive(Debug, Clone)]
pub struct CrateDetails {
    pub name: String,
    pub version: Version,
    pub deps: HashSet<String>,
    pub build_deps: HashSet<String>,
    pub dev_deps: HashSet<String>,
    pub should_be_published: bool,
    pub toml_path: PathBuf,
    pub readme: Option<String>,
}

impl CrateDetails {
    #[cfg(test)]
    pub fn new_for_testing(name: String) -> Self {
        Self {
            name,
            version: Version::new(0, 1, 0),
            deps: HashSet::new(),
            build_deps: HashSet::new(),
            dev_deps: HashSet::new(),
            should_be_published: true,
            toml_path: PathBuf::new(),
            readme: None,
        }
    }

    /// Read a Cargo.toml file, pulling out the information we care about.
    pub fn load(toml_path: PathBuf) -> anyhow::Result<CrateDetails> {
        let toml: toml_edit::Document = toml_read(&toml_path)?;

        let pkg = toml
            .get("package")
            .ok_or_else(|| anyhow!("Cannot read [package] section from {:?}", &toml_path))?;

        let name = pkg
            .get("name")
            .ok_or_else(|| anyhow!("Cannot read package.name from {:?}", &toml_path))?
            .as_str()
            .ok_or_else(|| anyhow!("Cannot read package.name as a string from {:?}", &toml_path))?
            .to_owned();

        let readme = if let Some(readme) = pkg.get("readme") {
            if let Some(readme) = readme.as_str() {
                Some(readme.to_owned())
            } else {
                anyhow::bail!(
                    "Cannot read package.readme as a string from {:?}",
                    &toml_path
                );
            }
        } else {
            None
        };

        let version = {
            let version = if let Some(version) = pkg.get("version") {
                if let Some(version) = version.as_str() {
                    version.to_owned()
                } else {
                    anyhow::bail!(
                        "Cannot read package.version as a string from {:?}",
                        &toml_path
                    );
                }
            } else {
                // Default to "0.1.0" for crates which don't have a particular
                // version, for example if the version is set through workspace
                // properties
                "0.1.0".to_owned()
            };
            Version::parse(&version)
                .with_context(|| format!("Cannot parse {version} as SemVer for {toml_path:?}"))?
        };

        let mut build_deps = HashSet::new();
        let mut dev_deps = HashSet::new();
        let mut deps = HashSet::new();
        for key in CrateDependencyKey::iter() {
            let key_name = &key.to_string();
            match key {
                CrateDependencyKey::Dependencies => {
                    for item in get_all_dependency_sections(&toml, key_name) {
                        deps.extend(filter_workspace_dependencies(&toml_path, key_name, item)?)
                    }
                }
                CrateDependencyKey::DevDependencies => {
                    for item in get_all_dependency_sections(&toml, key_name) {
                        dev_deps.extend(filter_workspace_dependencies(&toml_path, key_name, item)?)
                    }
                }
                CrateDependencyKey::BuildDependencies => {
                    for item in get_all_dependency_sections(&toml, key_name) {
                        build_deps
                            .extend(filter_workspace_dependencies(&toml_path, key_name, item)?)
                    }
                }
            }
        }

        let should_be_published = if let Some(value) = pkg.get("publish") {
            if let Some(value) = value.as_bool() {
                value
            } else {
                anyhow::bail!("Expected package.publish to be boolean in {:?}", &toml_path)
            }
        } else {
            true
        };

        Ok(CrateDetails {
            name,
            version,
            deps,
            dev_deps,
            build_deps,
            toml_path,
            should_be_published,
            readme,
        })
    }

    pub fn write_own_version(&mut self, new_version: Version) -> anyhow::Result<()> {
        // Load TOML file and update the version in that.
        let mut toml = self.read_toml()?;
        toml["package"]["version"] = toml_edit::value(new_version.to_string());
        self.write_toml(&toml)?;

        // If that worked, save the in-memory version too
        self.version = new_version;

        Ok(())
    }

    pub fn all_deps(&self) -> impl Iterator<Item = &String> {
        self.deps
            .iter()
            .chain(self.dev_deps.iter())
            .chain(self.build_deps.iter())
    }

    pub fn deps_to_publish(&self) -> impl Iterator<Item = &String> {
        self.deps.iter()
    }

    pub fn set_registry<S: AsRef<str>>(&self, registry: S) -> anyhow::Result<()> {
        let registry = registry.as_ref();

        let mut toml = self.read_toml()?;

        fn do_set(item: &mut toml_edit::Item, registry: &str) -> anyhow::Result<()> {
            let table = match item.as_table_like_mut() {
                Some(table) => table,
                None => return Ok(()),
            };

            for (_, item) in table.iter_mut() {
                if let Some(version) = item.as_str() {
                    let mut tbl = toml_edit::InlineTable::new();
                    tbl.insert("version", version.into());
                    tbl.insert("registry", registry.into());
                    *item = toml_edit::Item::Value(toml_edit::Value::InlineTable(tbl));
                } else {
                    item["registry"] = toml_edit::value(registry.to_string());
                }
            }

            Ok(())
        }

        for key in CrateDependencyKey::iter() {
            edit_all_dependency_sections(&mut toml, key, |item, _, _| do_set(item, registry))?;
        }

        self.write_toml(&toml)?;

        Ok(())
    }

    /// Set any references to the dependency provided to the version given.
    pub fn write_dependency_version(
        &self,
        dependency: &str,
        version: &Version,
        remove_dependency_path: bool,
    ) -> anyhow::Result<()> {
        if self.all_deps().any(|dep| dep == dependency) {
            write_dependency_version(&self.toml_path, dependency, version, remove_dependency_path)?;
        }
        Ok(())
    }

    pub fn strip_dev_deps<P>(&self, root: P) -> anyhow::Result<()>
    where
        P: AsRef<Path>,
    {
        let mut toml = self.read_toml()?;

        // Remove [dev-dependencies]
        let removed_top_level = toml.remove("dev-dependencies").is_some();
        // Remove [target.X.dev-dependencies]
        let removed_target_deps = toml
            .get_mut("target")
            .and_then(|item| item.as_table_like_mut())
            .into_iter()
            .flat_map(|table| table.iter_mut())
            .flat_map(|(_, item)| item.as_table_like_mut())
            .fold(false, |is_removed, t| {
                t.remove("dev-dependencies").is_some() || is_removed
            });

        if removed_top_level || removed_target_deps {
            with_git_checkpoint(
                &root,
                GitCheckpoint::RevertLater,
                || -> anyhow::Result<()> { self.write_toml(&toml) },
            )??;
        }

        Ok(())
    }

    pub fn publish(&self, verify: bool) -> Result<(), PublishError> {
        external::cargo::publish_crate(&self.name, &self.toml_path, verify)
    }

    pub fn adjust_version(&mut self, prev_versions: &[CratesIoCrateVersion]) -> anyhow::Result<()> {
        let highest_version = prev_versions
            .iter()
            .filter_map(|prev_version| {
                if prev_version.yanked {
                    None
                } else {
                    Some(&prev_version.version)
                }
            })
            .chain(vec![&self.version].into_iter())
            .max()
            .unwrap_or(&self.version);
        if highest_version != &self.version {
            self.write_own_version(highest_version.to_owned())?;
        }
        Ok(())
    }

    pub fn needs_publishing<P: AsRef<Path>>(&self, root: P) -> anyhow::Result<bool> {
        let result = self.needs_publishing_inner(&root, &self.version);
        git_checkpoint_revert(&root)?;
        result
    }

    fn needs_publishing_inner<P: AsRef<Path>>(
        &self,
        root: P,
        version: &semver::Version,
    ) -> anyhow::Result<bool> {
        let name = &self.name;

        let span = span!(Level::INFO, "__", crate = self.name);
        let _enter = span.enter();

        info!(
            "Comparing crate {} against crates.io to see if it needs to be published",
            self.name
        );

        self.strip_dev_deps(&root)?;

        let tmp_dir = tempfile::tempdir()?;
        let target_dir = if let Ok(tmp_dir) = env::var("SPUB_TMP") {
            PathBuf::from(tmp_dir)
        } else {
            tmp_dir.path().to_path_buf()
        };

        info!("Generating .crate file");
        let mut cmd = Command::new("cargo");
        if !cmd
            .arg("package")
            .arg("--manifest-path")
            .arg(&self.toml_path)
            .arg("--no-verify")
            .arg("--allow-dirty")
            .arg("--target-dir")
            .arg(&target_dir)
            .status()?
            .success()
        {
            anyhow::bail!("Failed to package crate {name}");
        };
        let pkg_path = target_dir
            .join("package")
            .join(format!("{name}-{}.crate", version));
        let pkg_bytes = fs::read(&pkg_path)?;

        info!("Checking generated .crate file against crates.io");
        let crates_io_bytes = if let Some(bytes) =
            external::crates_io::try_download_crate(&self.name, &self.version)?
        {
            bytes
        } else {
            return Ok(true);
        };

        if crates_io_bytes != pkg_bytes {
            info!("The file at {pkg_path:?} is different from the published version");
            return Ok(true);
        }

        info!("The crate is identical to the version from crates.io");
        Ok(false)
    }

    pub fn maybe_bump_version(
        &mut self,
        prev_versions: Vec<semver::Version>,
    ) -> anyhow::Result<bool> {
        let new_version = maybe_bump_for_breaking_change(prev_versions, self.version.clone());
        let bumped = if let Some(new_version) = new_version {
            info!(
                "Bumping crate {} from {} to {}",
                self.name, self.version, new_version
            );
            self.write_own_version(new_version)?;
            true
        } else {
            false
        };
        Ok(bumped)
    }

    fn read_toml(&self) -> anyhow::Result<toml_edit::Document> {
        toml_read(&self.toml_path)
    }

    fn write_toml(&self, toml: &toml_edit::Document) -> anyhow::Result<()> {
        toml_write(&self.toml_path, toml)
    }
}

/// An iterator that hands back all "dependencies"/"dev-dependencies"/"build-dependencies" (according to the
/// label provided), by looking in the top level `[label]` section as well as any `[target.'foo'.label]` sections.
fn get_all_dependency_sections<'a>(
    document: &'a toml_edit::Document,
    label: &'a str,
) -> impl Iterator<Item = &'a toml_edit::Item> + 'a {
    let target = document
        .get("target")
        .and_then(|t| t.as_table_like())
        .into_iter()
        .flat_map(|t| {
            // For each item of the "target" table, see if we can find a `label` section in it.
            t.iter()
                .flat_map(|(_name, item)| item.as_table_like())
                .flat_map(|t| t.get(label))
        });

    document.get(label).into_iter().chain(target)
}

/// Given a path to some dependencies in a TOML file, pull out the package names
/// for any path based dependencies (ie dependencies in the same workspace).
fn filter_workspace_dependencies<P: AsRef<Path>>(
    toml_path: P,
    key_name: &str,
    item: &toml_edit::Item,
) -> anyhow::Result<HashSet<String>> {
    let item = match item.as_table() {
        Some(item) => item,
        None => return Err(anyhow!("dependencies should be a TOML table.")),
    };

    let mut deps = HashSet::new();
    for (key, val) in item {
        let val = match val.as_table_like() {
            Some(val) => val,
            None => {
                if val.is_str() {
                    continue;
                } else {
                    anyhow::bail!(
                        "{} {} in {:?} should be specified as a string or table",
                        key_name,
                        key,
                        toml_path.as_ref(),
                    );
                }
            }
        };

        // Ignore any dependency without a "path" (not a workspace dep
        // if it doesn't point to another crate via a path).
        let path = match val.get("path") {
            Some(path) => path,
            None => continue,
        };

        // Expect path to be a string. Error if it's not.
        path.as_str().ok_or_else(|| {
            anyhow!(
                ".path field of {} {} in {:?} is not a string",
                key_name,
                key,
                toml_path.as_ref()
            )
        })?;

        // What is the actual package name?
        let package_name = val
            .get("package")
            .map(|package| {
                package.as_str().map(|s| s.to_string()).ok_or_else(|| {
                    anyhow!(
                        ".package field of {} {} in {:?} is not a string",
                        key_name,
                        key,
                        toml_path.as_ref()
                    )
                })
            })
            .unwrap_or_else(|| Ok(key.to_string()))?;

        deps.insert(package_name);
    }

    Ok(deps)
}
