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
use anyhow::Context;

use anyhow::anyhow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
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

    /// Does a crate need a version bump in order to publish?
    pub fn maybe_bump_crate_version<P: AsRef<Path>>(
        &mut self,
        krate: &str,
        root: P,
        bumped_versions: &mut HashMap<String, bool>,
    ) -> anyhow::Result<()> {
        if let Some(details) = self.details.get_mut(krate) {
            details.maybe_bump_version(root, bumped_versions)
        } else {
            anyhow::bail!("Crate not found: {krate}")
        }
    }

    /// return a list of the crates that will need publishing in order to ensure that the
    /// crates provided to this can be published in their current state.
    ///
    /// **Note:** it may be that one or more of the crate names provided are already
    /// published in their current state, in which case they won't be returned in the result.
    pub fn what_needs_publishing<K: AsRef<str>>(
        &mut self,
        krate: K,
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
                    .with_context(|| format!("Crate does not exist: {krate}"))?;

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
