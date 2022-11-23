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

use crate::crate_details::CrateDetails;
use crate::external;
use crate::git::*;
use crate::toml::toml_read;
use crate::toml::toml_write;
use anyhow::Context;
use std::fs;
use std::path::Path;
use strum::EnumString;

use anyhow::anyhow;
use std::collections::{HashMap, HashSet};

use std::path::PathBuf;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct Crates {
    root: PathBuf,
    // Details for a given crate, including dependencies.
    pub details: HashMap<String, CrateDetails>,
}

impl Crates {
    /// Return a map of all substrate crates, in the form `crate_name => ( path, details )`.
    pub fn load_crates_in_workspace(root: PathBuf) -> anyhow::Result<Crates> {
        // Load details:
        let details = crate_cargo_tomls(root.clone())
            .into_iter()
            .map(|path| {
                let details = CrateDetails::load(path)?;
                Ok((details.name.clone(), details))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        // Sanity check the details; make sure all listed dependencies exist.
        for crate_details in details.values() {
            for dep in &crate_details.deps {
                if !details.contains_key(dep) {
                    let crate_name = &crate_details.name;
                    return Err(anyhow!(
                        "{crate_name} contains workspace dependency {dep} which cannot be found"
                    ));
                }
            }
        }

        Ok(Crates { root, details })
    }

    pub fn setup_crates(&self) -> anyhow::Result<()> {
        for details in self.details.values() {
            // "cargo publish" *assumes* that each crate has a README.md without
            // if it doesn't specify README.md in its Cargo.toml, thus the
            // publish will always fail in case the crate doesn't have a README
            // file. To counteract that we'll a sample README.md file.
            if details.readme.is_none() {
                let crate_readme = details.toml_path.join("README.md");
                fs::write(
                    &crate_readme,
                    "Auto-generated README.md for publishing to crates.io",
                )?;
            }
        }
        Ok(())
    }

    /// Remove any dev-dependency sections in the TOML file and publish.
    pub fn strip_dev_deps_and_publish(&self, name: &str) -> anyhow::Result<()> {
        let details = match self.details.get(name) {
            Some(details) => details,
            None => anyhow::bail!("Crate '{name}' not found"),
        };

        details.strip_dev_deps(&self.root)?;
        details.publish()?;
        git_checkpoint_revert(&self.root)?;

        // Don't return until the crate has finished being published; it won't
        // be immediately visible on crates.io, so wait until it shows up.
        while !external::crates_io::does_crate_exist(name, &details.version)? {
            std::thread::sleep(std::time::Duration::from_millis(2500))
        }

        // Wait for the crate to be uploaded to the index after it is registered
        // on crates.io's database
        std::thread::sleep(std::time::Duration::from_millis(2500));

        Ok(())
    }

    pub fn what_needs_publishing<Crate: AsRef<str>>(
        &mut self,
        krate: Crate,
        publish_order: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let mut registered_crates: HashSet<&str> = HashSet::new();
        fn register_crates<'a>(
            crates: &'a Crates,
            registered_crates: &mut HashSet<&'a str>,
            krate: &'a str,
        ) -> anyhow::Result<()> {
            if registered_crates.get(krate).is_none() {
                registered_crates.insert(krate);

                let details = crates
                    .details
                    .get(krate)
                    .with_context(|| format!("Crate not found: {krate}"))?;

                for dep in details.deps_to_publish() {
                    register_crates(crates, registered_crates, dep)?;
                }
            }
            Ok(())
        }
        register_crates(self, &mut registered_crates, krate.as_ref())?;

        Ok(publish_order
            .iter()
            .filter_map(|krate| {
                if registered_crates.iter().any(|reg_crate| reg_crate == krate) {
                    Some(krate.into())
                } else {
                    None
                }
            })
            .collect())
    }
}

/// find all of the crates, returning paths to their Cargo.toml files.
fn crate_cargo_tomls(root: PathBuf) -> Vec<PathBuf> {
    let root_toml = {
        let mut p = root.clone();
        p.push("Cargo.toml");
        p
    };

    WalkDir::new(root)
        .into_iter()
        // Ignore hidden files and folders, and anything in "target" folders
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|s| !s.starts_with('.') && s != "target")
                .unwrap_or(false)
        })
        // Ignore errors
        .filter_map(|entry| entry.ok())
        // Keep files
        .filter(|entry| entry.file_type().is_file())
        // Keep files named Cargo.toml
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|s| s == "Cargo.toml")
                .unwrap_or(false)
        })
        // Filter the root Cargo.toml file
        .filter(|entry| entry.path() != root_toml)
        .map(|entry| entry.into_path())
        .collect()
}

#[derive(EnumString, strum::Display)]
pub enum CrateDependencyKey {
    #[strum(to_string = "build-dependencies")]
    BuildDependencies,
    #[strum(to_string = "dependencies")]
    Dependencies,
    #[strum(to_string = "dev-dependencies")]
    DevDependencies,
}
pub const CRATE_DEPENDENCY_KEYS: [CrateDependencyKey; 3] = [
    CrateDependencyKey::BuildDependencies,
    CrateDependencyKey::Dependencies,
    CrateDependencyKey::DevDependencies,
];

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

pub fn edit_all_dependency_sections<F: FnMut(&mut toml_edit::Item)>(
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

pub fn write_dependency_version<P: AsRef<Path>>(
    toml_path: P,
    dependency: &str,
    version: &semver::Version,
) -> anyhow::Result<()> {
    let mut toml = toml_read(&toml_path)?;

    fn do_set<P: AsRef<Path>>(
        item: &mut toml_edit::Item,
        version: &semver::Version,
        dep: &str,
        dep_type: &str,
        toml_path: P,
    ) -> anyhow::Result<()> {
        let table = match item.as_table_like_mut() {
            Some(table) => table,
            None => return Ok(()),
        };

        for (key, item) in table.iter_mut() {
            if key == dep {
                if item.is_str() {
                    *item = toml_edit::value(version.to_string());
                } else {
                    item["version"] = toml_edit::value(version.to_string());
                }
            } else {
                let item = if item.as_str().is_some() {
                    continue;
                } else {
                    item.as_table_like_mut().with_context(|| {
                        format!(
                            "{dep_type} 's key {key} should be a string or table-like in {:?}",
                            toml_path.as_ref().as_os_str()
                        )
                    })?
                };
                if item
                    .get("package")
                    .map(|pkg| pkg.as_str() == Some(dep))
                    .unwrap_or(false)
                {
                    item.insert("version", toml_edit::value(version.to_string()));
                }
            }
        }

        Ok(())
    }

    for dep_key in CRATE_DEPENDENCY_KEYS {
        let key = &dep_key.to_string();
        edit_all_dependency_sections(&mut toml, key, |item| {
            do_set(item, version, dependency, key, &toml_path).unwrap()
        });
    }

    toml_write(toml_path, &toml)?;

    Ok(())
}
