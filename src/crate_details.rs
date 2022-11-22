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

use crate::crates::{edit_all_dependency_sections, write_dependency_version, DEPENDENCIES_KEYS};
use crate::toml::{toml_read, toml_write};
use crate::version::bump_for_breaking_change;
use crate::{external, git::*};
use anyhow::{anyhow, Context};
use semver::Version;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, span, Level};

#[derive(Debug, Clone)]
pub struct CrateDetails {
    pub name: String,
    pub version: Version,
    pub deps: HashSet<String>,
    pub build_deps: HashSet<String>,
    pub dev_deps: HashSet<String>,
    pub should_be_published: bool,

    // Modifying the files on disk can only be done through the interface below.
    pub toml_path: PathBuf,
}

impl CrateDetails {
    /// Read a Cargo.toml file, pulling out the information we care about.
    pub fn load(path: PathBuf) -> anyhow::Result<CrateDetails> {
        let val: toml_edit::Document = toml_read(&path)?;

        let name = val
            .get("package")
            .ok_or_else(|| anyhow!("Cannot read [package] section from toml file."))?
            .get("name")
            .ok_or_else(|| anyhow!("Cannot read package.name from toml file."))?
            .as_str()
            .ok_or_else(|| anyhow!("package.name is not a string, but should be."))?
            .to_owned();

        let version = val
            .get("package")
            .ok_or_else(|| anyhow!("Cannot read [package] section from {name}."))?
            .get("version")
            .ok_or_else(|| anyhow!("Cannot read package.version from {name}."))?
            .as_str()
            .ok_or_else(|| anyhow!("Cannot read package.version from {name}."))?
            .to_owned();

        let version = Version::parse(&version)
            .with_context(|| format!("Cannot parse SemVer compatible version from {name}"))?;

        let mut build_deps = HashSet::new();
        let mut dev_deps = HashSet::new();
        let mut deps = HashSet::new();

        for key in DEPENDENCIES_KEYS {
            match key {
                "build-dependencies" => {
                    for item in get_all_dependency_sections(&val, key) {
                        build_deps.extend(filter_workspace_dependencies(item)?)
                    }
                }
                "dependencies" => {
                    for item in get_all_dependency_sections(&val, key) {
                        deps.extend(filter_workspace_dependencies(item)?)
                    }
                }
                "dev-dependencies" => {
                    for item in get_all_dependency_sections(&val, key) {
                        dev_deps.extend(filter_workspace_dependencies(item)?)
                    }
                }
                _ => anyhow::bail!("Dependency key is not handled: {}", key),
            }
        }

        let should_be_published = val
            .get("package")
            .ok_or_else(|| anyhow!("Cannot read [package] section from toml file."))?
            .get("publish")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);

        Ok(CrateDetails {
            name,
            version,
            deps,
            dev_deps,
            build_deps,
            toml_path: path,
            should_be_published,
        })
    }

    /// Bump the version for a breaking change and to release. Examples of bumps carried out:
    ///
    /// ```text
    /// 0.15.0 -> 0.16.0 (bump minor if 0.x.x)
    /// 4.0.0 -> 5.0.0 (bump major if >1.0.0)
    /// 4.0.0-dev -> 4.0.0 (remove prerelease label)
    /// 4.0.0+buildmetadata -> 5.0.0+buildmetadata (preserve build metadata regardless)
    /// ```
    ///
    /// Return the old and new version.
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

    pub fn deps_relevant_during_publish(&self) -> impl Iterator<Item = &String> {
        self.deps.iter().chain(self.build_deps.iter())
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
                    let mut table = toml_edit::table();
                    table["version"] = toml_edit::value(version.to_string());
                    table["registry"] = toml_edit::value(registry.to_string());
                    *item = table;
                } else {
                    item["registry"] = toml_edit::value(registry.to_string());
                }
            }

            Ok(())
        }

        for key in DEPENDENCIES_KEYS {
            edit_all_dependency_sections(&mut toml, key, |item| do_set(item, registry).unwrap());
        }

        self.write_toml(&toml)?;

        Ok(())
    }

    /// Set any references to the dependency provided to the version given.
    pub fn write_dependency_version(
        &self,
        dependency: &str,
        version: &Version,
    ) -> anyhow::Result<()> {
        if self.all_deps().any(|dep| dep == dependency) {
            write_dependency_version(&self.toml_path, dependency, version)?;
        }
        Ok(())
    }

    /// Strip dev dependencies.
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

        // Only write the toml file back if we did remove something.
        if removed_top_level || removed_target_deps {
            git_checkpoint(&root, GCM::Save)?;
            self.write_toml(&toml)?;
            git_checkpoint(&root, GCM::RevertLater)?;
        }

        Ok(())
    }

    /// Publish the current code for this crate as-is. You may want to run
    /// [`CrateDetails::strip_dev_deps()`] first.
    pub fn publish(&self) -> anyhow::Result<()> {
        let parent = self
            .toml_path
            .parent()
            .expect("parent of toml path should exist");
        external::cargo::publish_crate(parent, &self.name)
    }

    /// This checks whether we actually need to publish a new version of the crate. It'll return `false`
    /// only if, as far as we can see, the current version is published to crates.io, and there have been
    /// no changes to it since.
    pub fn needs_publishing<P: AsRef<Path>>(&self, root: P) -> anyhow::Result<bool> {
        let result = self.needs_publishing_inner(&root);
        git_checkpoint_revert(&root)?;
        result
    }

    pub fn needs_publishing_inner<P: AsRef<Path>>(&self, root: P) -> anyhow::Result<bool> {
        let name = &self.name;

        let span = span!(Level::INFO, "__", crate = self.name);
        let _enter = span.enter();

        info!(
            "Comparing crate {} against crates.io to see if it needs to be published",
            self.name
        );

        self.strip_dev_deps(&root)?;

        let crate_dir = self
            .toml_path
            .parent()
            .expect("parent of toml path should exist");

        let tmp_dir = tempfile::tempdir()?;
        let target_dir = if let Ok(tmp_dir) = std::env::var("SPUB_TMP") {
            PathBuf::from(tmp_dir)
        } else {
            tmp_dir.path().to_path_buf()
        };

        info!("Generating .crate file");
        let mut cmd = Command::new("cargo");
        if !cmd
            .current_dir(crate_dir)
            .arg("package")
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
            .join(format!("{name}-{}.crate", self.version));
        let pkg_bytes = std::fs::read(&pkg_path)?;

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

    pub fn maybe_bump_version(&mut self) -> anyhow::Result<bool> {
        let versions = external::crates_io::crate_versions(&self.name)?;
        let new_version = bump_for_breaking_change(versions, self.version.clone());
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
fn filter_workspace_dependencies(val: &toml_edit::Item) -> anyhow::Result<HashSet<String>> {
    let arr = match val.as_table() {
        Some(arr) => arr,
        None => return Err(anyhow!("dependencies should be a TOML table.")),
    };

    let mut deps = HashSet::new();
    for (name, props) in arr {
        // If props arent a table eg { path = "/foo" }, this is
        // not a workspace dependency (since it needs a "path" prop)
        // so skip over it.
        let props = match props.as_table_like() {
            Some(props) => props,
            None => continue,
        };

        // Ignore any dependency without a "path" (not a workspace dep
        // if it doesn't point to another crate via a path).
        let path = match props.get("path") {
            Some(path) => path,
            None => continue,
        };

        // Expect path to be a string. Error if it's not.
        path.as_str()
            .ok_or_else(|| anyhow!("{}.path is not a string.", name))?;

        // What is the actual package name?
        let package_name = props
            .get("package")
            .map(|package| {
                package
                    .as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow!("{}.package is not a string.", name))
            })
            .unwrap_or_else(|| Ok(name.to_string()))?;

        deps.insert(package_name);
    }

    Ok(deps)
}
