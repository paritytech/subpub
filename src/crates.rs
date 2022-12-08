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
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use anyhow::Context;
use strum::{EnumIter, EnumString, IntoEnumIterator};
use tracing::{info, warn};

use crate::{
    crate_details::CrateDetails,
    external::{self, cargo::PublishError},
    git::*,
    toml::{read_toml, write_toml},
};

pub type CrateName = String;
#[derive(Debug, Clone)]
pub struct Crates {
    pub root: PathBuf,
    pub crates_map: HashMap<CrateName, CrateDetails>,
}

impl Crates {
    pub fn load_workspace_crates(root: PathBuf) -> anyhow::Result<Crates> {
        let crates_map = {
            let cargo_tomls = workspace_cargo_tomls(&root)?;
            let mut crates_map: HashMap<String, CrateDetails> = HashMap::new();
            for cargo_toml in cargo_tomls {
                let details = CrateDetails::load(cargo_toml)?;
                if let Some(other_details) = crates_map.get(&details.name) {
                    anyhow::bail!(
                        "Crate parsed for {:?} has the same name of another crate parsed for {:?}",
                        details.toml_path,
                        other_details.toml_path,
                    );
                }
                crates_map.insert(details.name.clone(), details);
            }
            crates_map
        };

        // All path dependencies should be members of the root workspace so that
        // they can be verified and checked in tandem.
        for details in crates_map.values() {
            for dep in details.deps_to_publish() {
                if !crates_map.contains_key(dep) {
                    anyhow::bail!(
                      "Crate {} refers to path dependency {}, which could not be detected for the workspace of {:?}. You might need to add {} as a workspace member in {:?}.",
                      &details.name,
                      dep,
                      root.display(),
                      dep,
                      root.display()
                    );
                }
            }
        }

        Ok(Crates { root, crates_map })
    }

    pub fn setup(&self) -> anyhow::Result<()> {
        for details in self.crates_map.values() {
            // In case a crate does NOT define a `readme` field in its
            // `Cargo.toml`, `cargo publish` assumes, without first checking,
            // that a `README.md` file exists beside `Cargo.toml`. Publishing
            // will fail in case the crate doesn't comply with that assumption.
            // To work around that we'll crate a sample `README.md` file for
            // crates which don't specify or have one.
            if details.readme.is_none() {
                let crate_readme = details
                    .toml_path
                    .parent()
                    .with_context(|| format!("Failed to find parent of {:?}", details.toml_path))?
                    .join("README.md");
                if fs::metadata(&crate_readme).is_err() {
                    fs::write(
                        &crate_readme,
                        format!(
                            "# {}\n\nAuto-generated README.md for publishing to crates.io",
                            details.name
                        ),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Remove any dev-dependency sections in the TOML file and publish.
    pub fn publish(
        &self,
        krate: &String,
        crates_to_verify: &HashSet<&String>,
        after_publish_delay: Option<&u64>,
        last_publish_instant: &mut Option<Instant>,
    ) -> anyhow::Result<()> {
        let details = match self.crates_map.get(krate) {
            Some(details) => details,
            None => anyhow::bail!("Crate not found: {krate}"),
        };

        let should_verify = crates_to_verify.get(krate).is_some();

        info!("Stripping dev-dependencies of crate {krate} before publishing");
        details.strip_dev_deps(&self.root)?;

        if let Some(last_publish_instant) = last_publish_instant {
            if let Some(after_publish_delay) = after_publish_delay {
                let after_publish_delay = Duration::from_secs(*after_publish_delay);
                let now = last_publish_instant.elapsed();
                if now < after_publish_delay {
                    let sleep_duration = after_publish_delay - now;
                    info!(
                        "Waiting for {:?} before publishing crate {krate}",
                        sleep_duration
                    );
                    thread::sleep(sleep_duration);
                }
            };
        }

        info!("Publishing crate {krate}");
        while let Err(err) = details.publish(should_verify) {
            match err {
                PublishError::RateLimited(err) => {
                    info!("`cargo publish` failed due to rate limiting: {err}");
                    // crates.io should give a new token every 1 minute, so
                    // sleep by that much and try again
                    let rate_limit_delay = Duration::from_secs(64);
                    info!(
                        "Sleeping for {:?} before trying to publish again",
                        rate_limit_delay
                    );
                    thread::sleep(rate_limit_delay);
                }
                PublishError::Any(err) => {
                    return Err(err);
                }
            }
        }

        git_checkpoint_revert(&self.root)?;

        // Don't return until the crate has finished being published; it won't
        // be immediately visible on crates.io, so wait until it shows up.
        while !external::crates_io::does_crate_exist(krate, &details.version)? {
            thread::sleep(Duration::from_millis(2500))
        }

        *last_publish_instant = Some(Instant::now());

        if let Ok(crates_committed_file) = env::var("SPUB_CRATES_COMMITTED_FILE") {
            loop {
                let crates_committed =
                    fs::read_to_string(&crates_committed_file).with_context(|| {
                        format!(
                            "Failed to read $SPUB_CRATES_COMMITTED_FILE ({})",
                            crates_committed_file
                        )
                    })?;
                for crate_committed in crates_committed.lines() {
                    if crate_committed == krate {
                        return Ok(());
                    }
                }
                info!("Polling $SPUB_CRATES_COMMITTED_FILE for crate {krate}");
                thread::sleep(Duration::from_secs(2));
            }
        };

        Ok(())
    }

    pub fn what_needs_publishing<'a, Crate: AsRef<str>>(
        &self,
        krate: Crate,
        publish_order: &'a [String],
    ) -> anyhow::Result<Vec<&'a String>> {
        let mut registered_crates: HashSet<&str> = HashSet::new();
        fn register_crates<'b>(
            crates: &'b Crates,
            registered_crates: &mut HashSet<&'b str>,
            krate: &'b str,
        ) -> anyhow::Result<()> {
            if registered_crates.get(krate).is_none() {
                registered_crates.insert(krate);

                let details = crates
                    .crates_map
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
            .filter(|krate| registered_crates.iter().any(|reg_crate| reg_crate == krate))
            .collect())
    }
}

fn workspace_cargo_tomls(root: &PathBuf) -> anyhow::Result<Vec<PathBuf>> {
    if let Ok(metadata) = cargo_metadata::MetadataCommand::new()
        .current_dir(root)
        .exec()
    {
        return Ok(metadata
            .workspace_packages()
            .into_iter()
            .map(|pkg| pkg.manifest_path.as_std_path().to_path_buf())
            .collect());
    } else {
        warn!("cargo_metadata failed for workspace {root:?}; falling back to Git detection");
    }

    let mut cmd = Command::new("git");
    let cargo_tomls_output = cmd
        .current_dir(root)
        .arg("ls-files")
        .arg("--full-name")
        .arg("--exclude-standard")
        .arg("**/Cargo.toml")
        .output()?;
    if !cargo_tomls_output.status.success() {
        anyhow::bail!("Failed to run `git ls-files` for {root:?}",);
    }
    Ok(String::from_utf8(cargo_tomls_output.stdout.clone())
        .with_context(|| {
            format!(
                "Failed to parse output as UTF-8: {:?}\nBytes: {:?}",
                String::from_utf8_lossy(&cargo_tomls_output.stdout[..]),
                &cargo_tomls_output.stdout
            )
        })?
        .lines()
        .filter_map(|file_path| {
            if file_path.is_empty() {
                None
            } else {
                let mut root = root.clone();
                root.push(file_path);
                Some(root)
            }
        })
        .collect())
}

#[derive(EnumString, strum::Display, EnumIter, PartialEq, Eq)]
pub enum CrateDependencyKey {
    #[strum(to_string = "build-dependencies")]
    BuildDependencies,
    #[strum(to_string = "dependencies")]
    Dependencies,
    #[strum(to_string = "dev-dependencies")]
    DevDependencies,
}

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

pub fn edit_all_dependency_sections<
    T,
    F: FnMut(&mut toml_edit::Item, &CrateDependencyKey, &str) -> anyhow::Result<T>,
>(
    document: &mut toml_edit::Document,
    dep_key: CrateDependencyKey,
    mut f: F,
) -> anyhow::Result<()> {
    let dep_key_display = dep_key.to_string();
    if let Some(item) = document.get_mut(&dep_key_display) {
        f(item, &dep_key, &dep_key_display)?;
    }
    for item in get_target_dependency_sections_mut(document, &dep_key_display) {
        f(item, &dep_key, &dep_key_display)?;
    }
    Ok(())
}

pub fn write_dependency_version<P: AsRef<Path>>(
    toml_path: P,
    dependency: &str,
    version: &semver::Version,
    // Removing the dependencies' paths is useful for verifying that they can be
    // consumed from the registry after publishing.
    remove_dependency_path: bool,
) -> anyhow::Result<()> {
    let mut toml = read_toml(&toml_path)?;

    fn visit<P: AsRef<Path>>(
        item: &mut toml_edit::Item,
        version: &semver::Version,
        dep: &str,
        dep_key_display: &str,
        toml_path: P,
        remove_dependency_path: bool,
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
                    let item = item.as_table_like_mut().with_context(|| {
                        format!(
                            "{}.{} should be a string or table-like in {:?}",
                            dep_key_display,
                            key,
                            toml_path.as_ref().as_os_str()
                        )
                    })?;
                    item.insert("version", toml_edit::value(version.to_string()));
                    if remove_dependency_path {
                        item.remove("path");
                    }
                }
            } else {
                let item = if item.as_str().is_some() {
                    continue;
                } else {
                    item.as_table_like_mut().with_context(|| {
                        format!(
                            "{}.{} should be a string or table-like in {:?}",
                            dep_key_display,
                            key,
                            toml_path.as_ref().as_os_str()
                        )
                    })?
                };
                let pkg = if let Some(pkg) = item.get("package") {
                    pkg
                } else {
                    continue;
                };
                if let Some(pkg) = pkg.as_str() {
                    if pkg == dep {
                        item.insert("version", toml_edit::value(version.to_string()));
                        if remove_dependency_path {
                            item.remove("path");
                        }
                    }
                } else {
                    anyhow::bail!(
                        "{}.{}.package should be a string in {:?}",
                        dep_key_display,
                        key,
                        toml_path.as_ref().as_os_str()
                    );
                }
            }
        }

        Ok(())
    }

    for dep_key in CrateDependencyKey::iter() {
        edit_all_dependency_sections(&mut toml, dep_key, |item, _, dep_key_display| {
            visit(
                item,
                version,
                dependency,
                dep_key_display,
                &toml_path,
                remove_dependency_path,
            )
        })?;
    }

    write_toml(toml_path, &toml)?;

    Ok(())
}
