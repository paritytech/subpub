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

use std::{path::{Path, PathBuf}, io::{Read, Cursor}};
use anyhow::{anyhow, Context};
use semver::Version;
use std::collections::HashSet;
use crate::crates_io;

#[derive(Debug, Clone)]
pub struct CrateDetails {
    pub name: String,
    pub version: Version,
    pub deps: HashSet<String>,
    pub build_deps: HashSet<String>,
    pub dev_deps: HashSet<String>,

    // Modifying the files on disk can only be done through the interface below.
    toml_path: PathBuf,
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

        for item in get_target_dependencies(&val, "build-dependencies") {
            build_deps.extend(filter_workspace_dependencies(item)?);
        }
        for item in get_target_dependencies(&val, "dev-dependencies") {
            dev_deps.extend(filter_workspace_dependencies(item)?);
        }
        for item in get_target_dependencies(&val, "dependencies") {
            deps.extend(filter_workspace_dependencies(item)?);
        }

        Ok(CrateDetails {
            name,
            version,
            deps,
            dev_deps,
            build_deps,
            toml_path: path,
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
    pub fn write_own_version(&mut self, version: Version) -> anyhow::Result<()> {
        // Load TOML file and update the version in that.
        let mut toml = self.read_toml()?;
        toml["package"]["version"] = toml_edit::value(version.to_string());
        self.write_toml(&toml)?;

        // If that worked, save the in-memory version too
        self.version = version;

        Ok(())
    }

    /// Set any references to the dependency provided to the version given.
    pub fn write_dependency_version(&self, dependency: &str, version: &Version) -> anyhow::Result<bool> {
        if !self.build_deps.contains(dependency)
        && !self.dev_deps.contains(dependency)
        && !self.deps.contains(dependency) {
            return Ok(false)
        }

        let mut toml = self.read_toml()?;

        fn do_set(item: &mut toml_edit::Item, version: &Version, dependency: &str) {
            let table = match item.as_table_like_mut() {
                Some(table) => table,
                None => return
            };

            let dep = match table.get_mut(dependency) {
                Some(dep) => dep,
                None => return
            };

            if dep.is_str() {
                // Set version if it's just a string
                *dep = toml_edit::value(version.to_string());
            } else if let Some(table) = dep.as_table_like_mut() {
                // If table, only update version if version is present
                if let Some(v) = table.get_mut("version") {
                    *v = toml_edit::value(version.to_string());
                }
            }
        }

        // TODO: can we do a mut version of get_target_dependencies??
        for (name, item) in toml.iter_mut() {
            if is_dependency_section(&name, "build-dependencies") {
                do_set(item, version, dependency);
            } else if is_dependency_section(&name, "dev-dependencies") {
                do_set(item, version, dependency);
            } else if is_dependency_section(&name, "dependencies") {
                do_set(item, version, dependency);
            }
        }
        self.write_toml(&toml)?;

        Ok(true)
    }

    /// Strip dev dependencies.
    pub fn strip_dev_deps(&self) -> anyhow::Result<()> {
        let mut toml = self.read_toml()?;
        if toml.remove("dev-dependencies").is_some() {
            // Only write the file if the remove actually did something, otherwise just leave it.
            self.write_toml(&toml)?;
        }
        Ok(())
    }

    /// This checks whether we actually need to publish a new version of the crate. It'll return `false`
    /// only if, as far as we can see, the current version is published to crates.io, and there have been
    /// no changes to it since.
    pub fn needs_publishing(&self) -> anyhow::Result<bool> {
        let name = &self.name;

        let crate_bytes = crates_io::try_download_crate(&self.name, &self.version)
            .with_context(|| format!("Could not download crate {name}"))?;

        let crate_bytes = match crate_bytes {
            Some(bytes) => bytes,
            None => {
                // crate at current version doesn't exist; this def needs publishing, then.
                // Especially useful since when we bump the version we'll end up in this branch
                // which will be quicker.
                return Ok(true)
            }
        };

        // Crates on crates.io are gzipped tar files, so uncompress before decoding the archive.
        let crate_bytes = flate2::read::GzDecoder::new(Cursor::new(crate_bytes));
        let mut archive = tar::Archive::new(crate_bytes);
        let entries = archive.entries()
            .with_context(|| format!("Could not read files in published crate {name}"))?;

        // Root path on disk to compare with.
        let crate_root = self.toml_path.parent().expect("should always exist");

        for entry in entries {
            let entry = entry
                .with_context(|| format!("Could not read files in published crate {name}"))?;

            // Get the path of the current archive entry
            let path = entry.path()
                .with_context(|| format!("Could not read path for crate {name}"))?
                .into_owned();

            // Build a path given this and the root path to find the file to compare against.
            let mut path = {
                // Strip the beginning from the archive path
                let mut components = path.components();
                components.next();

                // Join these to the crate root:
                let root_path = crate_root.to_path_buf();
                root_path.join(components.as_path())
            };

            // Ignore the auto-generated "Cargo.toml" file in the crate
            if path.ends_with("Cargo.toml") {
                continue
            }

            // Ignore this auto-generated file, too
            if path.ends_with(".cargo_vcs_info.json") {
                continue
            }

            // Compare the file Cargo.toml.orig against our Cargo.toml
            if path.ends_with("Cargo.toml.orig") {
                path.set_file_name("Cargo.toml");
            }

            let file = match std::fs::File::open(&path) {
                // Can't find file that's in crate? needs publishing.
                Err(_e) => {
                    log::debug!("{name}: a file at {path:?} is published but does not exist locally");
                    return Ok(true)
                }
                Ok(f) => f
            };

            if !are_contents_equal(file, entry)? {
                log::debug!("{name}: the file at {path:?} is different from the published version");
                return Ok(true)
            }
        }

        // We compared all files and they all came up equal,
        // so no need to publish this.
        log::debug!("{name}: this crate is identical to the published version");
        Ok(false)
    }

    /// Does this create need a version bump in order to be published?
    pub fn needs_version_bump_to_publish(&self) -> anyhow::Result<bool> {
        if self.version.pre != semver::Prerelease::EMPTY {
            // If prerelease eg `-dev`, we'll want to bump.
            return Ok(true)
        }

        // Does the current version of this crate exist on crates.io?
        // If so, we need to bump the current version.
        let known_versions = crates_io::get_known_crate_versions(&self.name)?;
        Ok(known_versions.contains(&self.version))
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

fn get_target_dependencies<'a>(document: &'a toml_edit::Document, label: &'a str) -> impl Iterator<Item = &'a toml_edit::Item> + 'a {
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

    document
        .get(label)
        .into_iter()
        .chain(target)
}

// Load a TOML file.
fn read_toml(path: &Path) -> anyhow::Result<toml_edit::Document> {
    let toml_string = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read the Cargo.toml at {path:?}"))?;
    let toml = toml_string.parse::<toml_edit::Document>()
        .with_context(|| format!("Cannot parse the Cargo.toml at {path:?}"))?;
    Ok(toml)
}

/// Given a path to some dependencies in a TOML file, pull out the package names
/// for any path based dependencies (ie dependencies in the same workspace).
fn filter_workspace_dependencies(val: &toml_edit::Item) -> anyhow::Result<HashSet<String>> {
    let arr = match val.as_table() {
        Some(arr) => arr,
        None => return Err(anyhow!("dependencies should be a TOML table."))
    };

    let mut deps = HashSet::new();
    for (name, props) in arr {
        // If props arent a table eg { path = "/foo" }, this is
        // not a workspace dependency (since it needs a "path" prop)
        // so skip over it.
        let props = match props.as_table_like() {
            Some(props) => props,
            None => continue
        };

        // Ignore any dependency without a "path" (not a workspace dep
        // if it doesn't point to another crate via a path).
        let path = match props.get("path") {
            Some(path) => path,
            None => continue
        };

        // Expect path to be a string. Error if it's not.
        path
            .as_str()
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

/// Compare the content of 2 readers, returning whether they are equal or not.
// Note: This could be optimised a fair bit.
fn are_contents_equal<A: Read, B: Read>(mut a: A, mut b: B) -> anyhow::Result<bool> {
    let mut a_vec = vec![];
    a.read_to_end(&mut a_vec)?;

    let mut b_vec = vec![];
    b.read_to_end(&mut b_vec)?;

    // if a_vec != b_vec {
    //     let a = std::str::from_utf8(&a_vec).unwrap();
    //     let b = std::str::from_utf8(&b_vec).unwrap();
    //     println!("DIFFERENT:\n\n{a}\n\n###################################\n\n{b}");
    // }

    Ok(a_vec == b_vec)
}