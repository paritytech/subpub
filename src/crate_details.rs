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

use crate::version::bump_for_breaking_change;
use crate::{external, git::*};
use anyhow::{anyhow, Context};
use semver::Version;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        let val: toml_edit::Document = read_toml(&path)?;

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

        for item in get_all_dependency_sections(&val, "build-dependencies") {
            build_deps.extend(filter_workspace_dependencies(item)?);
        }
        for item in get_all_dependency_sections(&val, "dev-dependencies") {
            dev_deps.extend(filter_workspace_dependencies(item)?);
        }
        for item in get_all_dependency_sections(&val, "dependencies") {
            deps.extend(filter_workspace_dependencies(item)?);
        }

        let published = val
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
            should_be_published: published,
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

    fn all_deps(&self) -> impl Iterator<Item = &String> {
        self.deps
            .iter()
            .chain(self.dev_deps.iter())
            .chain(self.build_deps.iter())
    }

    /// Set any references to the dependency provided to the version given.
    pub fn write_dependency_version(
        &self,
        dependency: &str,
        version: &Version,
    ) -> anyhow::Result<bool> {
        if !self.all_deps().any(|dep| dep == dependency) {
            return Ok(true);
        }

        let mut toml = self.read_toml()?;

        fn do_set<'a>(
            item: &mut toml_edit::Item,
            version: &Version,
            dep: &str,
            dep_type: &str,
            toml_path: &PathBuf,
        ) -> anyhow::Result<()> {
            let table = match item.as_table_like_mut() {
                Some(table) => table,
                None => return Ok(()),
            };

            for (key, item) in table.iter_mut() {
                if key == dep {
                    if item.is_str() {
                        if let Ok(registry) = std::env::var("SPUB_REGISTRY") {
                            *item = toml_edit::value(version.to_string());
                            let mut table = toml_edit::table();
                            table["version"] = toml_edit::value(version.to_string());
                            table["registry"] = toml_edit::value(registry.to_string());
                            *item = table;
                        } else {
                            *item = toml_edit::value(version.to_string());
                        }
                    } else {
                        item["version"] = toml_edit::value(version.to_string());
                        if let Ok(registry) = std::env::var("SPUB_REGISTRY") {
                            item["registry"] = toml_edit::value(registry.to_string());
                        }
                    }
                } else {
                    let item = if item.as_str().is_some() {
                        continue;
                    } else {
                        item.as_table_like_mut().with_context(|| {
                            format!(
                                "{dep_type} {key} should be a string or table-like in {:?}",
                                toml_path
                            )
                        })?
                    };
                    if item
                        .get("package")
                        .map(|pkg| pkg.as_str() == Some(dep))
                        .unwrap_or(false)
                    {
                        item.insert("version", toml_edit::value(version.to_string()));
                        if let Ok(registry) = std::env::var("SPUB_REGISTRY") {
                            item.insert("registry", toml_edit::value(registry.to_string()));
                        }
                    }
                }
            }

            Ok(())
        }

        edit_all_dependency_sections(&mut toml, "build-dependencies", |item| {
            do_set(
                item,
                version,
                dependency,
                "build-dependency",
                &self.toml_path,
            )
            .unwrap()
        });
        edit_all_dependency_sections(&mut toml, "dev-dependencies", |item| {
            do_set(item, version, dependency, "dev-dependency", &self.toml_path).unwrap()
        });
        edit_all_dependency_sections(&mut toml, "dependencies", |item| {
            do_set(item, version, dependency, "dependency", &self.toml_path).unwrap()
        });

        self.write_toml(&toml)?;

        Ok(true)
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

        println!("[{}] Generating .crate file", self.name);
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

        println!(
            "[{}] Checking generated .crate file against crates.io",
            self.name
        );
        let crates_io_bytes = if let Some(bytes) =
            external::crates_io::try_download_crate(&self.name, &self.version)?
        {
            bytes
        } else {
            return Ok(true);
        };

        if crates_io_bytes != pkg_bytes {
            log::debug!(
                "[{name}] the file at {pkg_path:?} is different from the published version"
            );
            return Ok(true);
        }

        log::debug!("[{name}] this crate is identical to the version from crates.io");
        Ok(false)
    }

    /// Does this create need a version bump in order to be published?
    pub fn maybe_bump_version<P: AsRef<Path>>(
        &mut self,
        _root: P,
        bumped_versions: &mut HashMap<String, bool>,
    ) -> anyhow::Result<()> {
        if bumped_versions.get(&self.name).is_none() {
            let versions = external::crates_io::crate_versions(&self.name)?;
            let new_version = bump_for_breaking_change(versions, self.version.clone());
            if let Some(new_version) = new_version {
                println!("Bumping crate from {} to {}", self.version, new_version);
                self.write_own_version(new_version)?;
                for _dep in self.all_deps() {
                    self.write_dependency_version(&self.name, &self.version)?;
                }
            }
            bumped_versions.insert((&self.name).into(), true);
        };
        Ok(())
    }

    fn read_toml(&self) -> anyhow::Result<toml_edit::Document> {
        read_toml(&self.toml_path)
    }

    fn write_toml(&self, toml: &toml_edit::Document) -> anyhow::Result<()> {
        let name = &self.name;
        std::fs::write(&self.toml_path, &toml.to_string())
            .with_context(|| format!("Cannot save the updated Cargo.toml for {name}"))?;
        Ok(())
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

/// Similar to `get_all_dependencies`, but mutable iterates over just `[target.'foo'.label]` sections.
fn get_target_dependency_sections_mut<'a>(
    document: &'a mut toml_edit::Document,
    label: &'a str,
) -> impl Iterator<Item = &'a mut toml_edit::Item> + 'a {
    document
        .get_mut("target")
        .and_then(|t| t.as_table_like_mut())
        .into_iter()
        .flat_map(|t| {
            // For each item of the "target" table, see if we can find a `label` section in it.
            t.iter_mut()
                .flat_map(|(_name, item)| item.as_table_like_mut())
                .flat_map(|t| t.get_mut(label))
        })
}

/// Allows a function to be provided that is passed each mutable `Item` we find when searching for
/// "dependencies"/"dev-dependencies"/"build-dependencies".
fn edit_all_dependency_sections<F: FnMut(&mut toml_edit::Item)>(
    document: &mut toml_edit::Document,
    label: &str,
    mut f: F,
) {
    if let Some(item) = document.get_mut(label) {
        f(item);
    }
    for item in get_target_dependency_sections_mut(document, label) {
        f(item)
    }
}

// Load a TOML file.
fn read_toml(path: &Path) -> anyhow::Result<toml_edit::Document> {
    let toml_string = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read the Cargo.toml at {path:?}"))?;
    let toml = toml_string
        .parse::<toml_edit::Document>()
        .with_context(|| format!("Cannot parse the Cargo.toml at {path:?}"))?;
    Ok(toml)
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
            .unwrap_or(Ok(name.to_string()))?;

        deps.insert(package_name);
    }

    Ok(deps)
}
