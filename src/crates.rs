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

use std::{path::PathBuf};
use walkdir::WalkDir;
use anyhow::anyhow;
use std::collections::{ HashMap, HashSet };
use crate::crate_details::{ CrateDetails };
use crate::version::{ Version, bump_for_breaking_change };
use crate::external;

#[derive(Debug, Clone)]
pub struct Crates {
    root: PathBuf,
    // Details for a given crate, including dependencies.
    details: HashMap<String, CrateDetails>,
    // Which crates depend on a given crate.
    dependees: HashMap<String, Dependees>
}

#[derive(Debug, Clone, Default)]
struct Dependees {
    deps: HashSet<String>,
    build_deps: HashSet<String>,
    dev_deps: HashSet<String>,
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
            .collect::<anyhow::Result<HashMap<_,_>>>()?;

        // Sanity check the details; make sure all listed dependencies exist.
        for crate_details in details.values() {
            for dep in &crate_details.deps {
                if !details.contains_key(dep) {
                    let crate_name = &crate_details.name;
                    return Err(anyhow!("{crate_name} contains workspace dependency {dep} which cannot be found"))
                }
            }
        }

        // Build a reverse dependency map, since it's useful to know which crates
        // depend on a given crate in our workspace.
        let mut dependees: HashMap<String, Dependees> = details
            .keys()
            .map(|s| (s.clone(), Dependees::default()))
            .collect();
        for crate_details in details.values() {
            for dep in &crate_details.deps {
                dependees.entry(dep.clone()).or_default().deps.insert(crate_details.name.clone());
            }
            for dep in &crate_details.build_deps {
                dependees.entry(dep.clone()).or_default().build_deps.insert(crate_details.name.clone());
            }
            for dep in &crate_details.dev_deps {
                dependees.entry(dep.clone()).or_default().dev_deps.insert(crate_details.name.clone());
            }
        }

        Ok(Crates {
            root,
            details,
            dependees,
        })
    }

    /// Update the lockfile for the crates given and any of their dependencies if they've changed.
    pub fn update_lockfile_for_crates<'a, I, S>(&self, crates: I) -> anyhow::Result<()>
    where
        S: AsRef<str>,
        I: IntoIterator<Item=S> + Clone
    {
        for name in crates.clone().into_iter() {
            let name = name.as_ref();
            if !self.details.contains_key(name) {
                anyhow::bail!("Crate `{name}` not found");
            }
        }

        external::cargo::update_lockfile_for_crates(&self.root, crates)
    }

    /// Remove any dev-dependency sections in the TOML file and publish.
    pub fn strip_dev_deps_and_publish(&self, name: &str) -> anyhow::Result<()> {
        let details = match self.details.get(name) {
            Some(details) => details,
            None => anyhow::bail!("Crate '{name}' not found")
        };

        details.strip_dev_deps()?;
        details.publish()?;

        // Don't return until the crate has finished being published; it won't
        // be immediately visible on crates.io, so wait until it shows up.
        while !external::crates_io::does_crate_exist(name, &details.version)? {
            std::thread::sleep(std::time::Duration::from_millis(2500))
        }

        Ok(())
    }

    /// Does a crate need a version bump in order to publish?
    pub fn does_crate_version_need_bumping_to_publish(&self, name: &str) -> anyhow::Result<bool> {
        let details = match self.details.get(name) {
            Some(details) => details,
            None => anyhow::bail!("Crate '{name}' not found")
        };

        details.needs_version_bump_to_publish()
    }

    /// Bump the version of the crate given, and update it in all dependant crates as needed.
    /// Return the old version and the new version.
    pub fn bump_crate_version_for_breaking_change(&mut self, name: &str) -> anyhow::Result<(Version, Version)> {
        let details = match self.details.get_mut(name) {
            Some(details) => details,
            None => anyhow::bail!("Crate '{name}' not found")
        };

        let old_version = details.version.clone();
        let new_version = bump_for_breaking_change(old_version.clone());

        // Bump the crate version:
        details.write_own_version(new_version.clone())?;

        // Find any crate which depends on this crate and bump the version there too.
        for details in self.details.values() {
            details.write_dependency_version(name, &new_version)?;
        }

        Ok((old_version, new_version))
    }

    /// return a list of the crates that will need publishing in order to ensure that the
    /// crates provided to this can be published in their current state.
    ///
    /// **Note:** it may be that one or more of the crate names provided are already
    /// published in their current state, in which case they won't be returned in the result.
    pub fn what_needs_publishing(&self, crates: Vec<String>) -> anyhow::Result<Vec<String>> {

        struct Details<'a> {
            dependees: HashSet<String>,
            details: &'a CrateDetails,
            depth: usize,
            needs_publishing: bool
        }
        let mut tree: HashMap<String, Details> = HashMap::new();

        // Step 1: make a note of the crates we care about based on the names
        // provided, which are the ones we ultimately want to be published
        // in their current state.

        fn note_crates<'a>(
            all: &'a Crates,
            tree: &mut HashMap<String, Details<'a>>,
            crates: impl IntoIterator<Item=String>,
            depth: usize
        ) {
            for name in crates {
                let details = match all.details.get(&name) {
                    Some(details) => details,
                    // Crate doesn't exist; ignore it.
                    None => continue
                };

                let entry = tree.entry(name).or_insert_with(|| Details {
                    dependees: HashSet::new(),
                    details,
                    depth,
                    needs_publishing: false,
                });

                // We care about the deepest depth we find, so update as needed.
                if entry.depth < depth {
                    entry.depth = depth
                }
                // Recurse and add all dependencies to our tree, too. We need to check whether
                // any of those need publishing as well.
                note_crates(all, tree, details.deps.iter().cloned(), depth + 1);
            }
        }
        note_crates(self, &mut tree, crates, 0);

        // Step 2: populate the dependees for each crate. We pay attention to deps and
        // build deps but ignore dev deps, since those are irrelevant for publishing.
        // We need to wait until we have our entire sub-tree to do this, built above.

        fn populate_dependees<'a>(
            all: &'a Crates,
            tree: &mut HashMap<String, Details<'a>>,
        ) {
            let crates_in_sub_tree: HashSet<String> = tree.keys().cloned().collect();
            for (name, details) in tree.iter_mut() {
                let dependees = all.dependees
                    .get(name)
                    .expect("should exist");

                details.dependees = dependees.build_deps
                    .union(&dependees.deps)
                    .cloned()
                    .filter(|d| crates_in_sub_tree.contains(d))
                    .collect();
            }
        }
        populate_dependees(self, &mut tree);

        // Step 3: work out which of the crates in this graph need publishing. We work
        // from the deepest dependencies up, and for each dependency we find that needs
        // publishing, we mark all crates that depend on it as also needing publishing.

        fn set_needs_publishing(tree: &mut HashMap<String, Details>, name: &str) {
            let entry = tree
                .get_mut(name)
                .expect("should exist");

            entry.needs_publishing = true;
            for dep in entry.dependees.clone().iter() {
                set_needs_publishing(tree, dep);
            }
        }

        let mut deepest_first: Vec<(String, usize)> = tree
            .iter()
            .map(|(name, details)| (name.clone(), details.depth))
            .collect();
        deepest_first.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));

        for (name, _) in deepest_first.iter() {
            let details = tree
                .get_mut(name)
                .expect("should exist");

            // Ignore things that we already know need publishing
            if details.needs_publishing {
                continue
            }

            // If the crate itself needs publishing, mark it and anything
            // depending on it as needing publishing.
            if details.details.needs_publishing()? {
                set_needs_publishing(&mut tree, name);
            }
        }

        // Step 4: Return a filtered list of crates we need to bump versions/publish
        // in order to publish the crates originally provided. Return the list im the
        // order that you'd need to publish them.

        let crates_that_need_publishing = deepest_first
            .into_iter()
            .map(|(name, _depth)| name)
            .filter(|name| tree.get(name).unwrap().needs_publishing)
            .collect();

        Ok(crates_that_need_publishing)
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
                .map(|s| !s.starts_with(".") && s != "target")
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

