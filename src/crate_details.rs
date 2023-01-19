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
use cargo_metadata::Package;
use external::crates_io::CratesIoCrateVersion;
use semver::Version;
use strum::IntoEnumIterator;
use tempfile::TempDir;
use tracing::{info, span, Level};

use crate::{
    dependencies::{
        edit_all_dependency_sections, write_dependency_field_value, CrateDependencyKey,
    },
    external::{self, cargo::PublishError},
    git::*,
    toml::{read_toml, write_toml},
    version::{
        maybe_bump_for_breaking_change, maybe_bump_for_compatible_change, VersionBumpHeuristic,
    },
};

#[derive(Debug, Clone)]
pub struct CrateDetails {
    pub name: String,
    pub version: Version,
    pub deps: HashSet<String>,
    pub build_deps: HashSet<String>,
    pub dev_deps: HashSet<String>,
    pub should_be_published: bool,
    pub manifest_path: PathBuf,
    pub readme: Option<PathBuf>,
}

impl CrateDetails {
    #[cfg(feature = "test-0")]
    pub fn new_for_testing(name: String) -> Self {
        Self {
            name,
            version: Version::new(0, 1, 0),
            deps: HashSet::new(),
            build_deps: HashSet::new(),
            dev_deps: HashSet::new(),
            should_be_published: true,
            manifest_path: PathBuf::new(),
            readme: None,
        }
    }

    pub fn load(pkg: &Package) -> anyhow::Result<CrateDetails> {
        let manifest_path = &pkg.manifest_path;

        let toml: toml_edit::Document = read_toml(manifest_path)?;

        let mut build_deps = HashSet::new();
        let mut dev_deps = HashSet::new();
        let mut deps = HashSet::new();
        for key in CrateDependencyKey::iter() {
            let key_name = &key.to_string();
            match key {
                CrateDependencyKey::Dependencies => {
                    for item in get_all_dependency_sections(&toml, key_name) {
                        deps.extend(filter_path_dependencies(manifest_path, key_name, item)?)
                    }
                }
                CrateDependencyKey::DevDependencies => {
                    for item in get_all_dependency_sections(&toml, key_name) {
                        dev_deps.extend(filter_path_dependencies(manifest_path, key_name, item)?)
                    }
                }
                CrateDependencyKey::BuildDependencies => {
                    for item in get_all_dependency_sections(&toml, key_name) {
                        build_deps.extend(filter_path_dependencies(manifest_path, key_name, item)?)
                    }
                }
            }
        }

        let should_be_published = match pkg.publish.as_ref() {
            Some(registries) => !registries.is_empty(),
            None => true,
        };

        Ok(CrateDetails {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            deps,
            dev_deps,
            build_deps,
            manifest_path: pkg.manifest_path.clone().into(),
            should_be_published,
            readme: pkg.readme.as_ref().map(|readme| readme.clone().into()),
        })
    }

    pub fn write_own_version(&mut self, new_version: Version) -> anyhow::Result<()> {
        // Load TOML file and update the version in that.
        let mut manifest = self.read_manifest()?;
        manifest["package"]["version"] = toml_edit::value(new_version.to_string());
        self.write_toml(&manifest)?;

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

        let mut manifest = self.read_manifest()?;

        fn visit(
            item: &mut toml_edit::Item,
            registry: &str,
            dep_key: &CrateDependencyKey,
            dep_key_display: &str,
            manifest_path: &PathBuf,
        ) -> anyhow::Result<()> {
            let deps = item.as_table_like_mut().with_context(|| {
                format!(
                    ".{} should be table-like in {:?}",
                    dep_key_display, manifest_path
                )
            })?;

            for (key, value) in deps.iter_mut() {
                if let Some(version) = value.as_str() {
                    let mut tbl = toml_edit::InlineTable::new();
                    tbl.insert("version", version.into());
                    tbl.insert("registry", registry.into());
                    *value = toml_edit::Item::Value(toml_edit::Value::InlineTable(tbl));
                } else {
                    let item = value.as_table_like_mut().with_context(|| {
                        format!(
                            ".{}.{} should be a string or table-like in {:?}",
                            dep_key_display, key, manifest_path
                        )
                    })?;
                    item.insert("registry", toml_edit::value(registry.to_string()));
                    for key in &["git", "branch", "tag", "rev"] {
                        item.remove(key);
                    }
                }
            }

            Ok(())
        }

        for dep_key in CrateDependencyKey::iter() {
            edit_all_dependency_sections(&mut manifest, &dep_key, |item, _, dep_key_display| {
                visit(
                    item,
                    registry,
                    &dep_key,
                    dep_key_display,
                    &self.manifest_path,
                )
            })?;
        }

        self.write_toml(&manifest)?;

        Ok(())
    }

    /// Set any references to the dependency provided to the version given.
    pub fn write_dependency_version(
        &self,
        dep: &str,
        version: &Version,
        // Removing the dependencies' paths is useful for verifying that they can be
        // consumed from the registry after publishing.
        remove_dep_path: bool,
    ) -> anyhow::Result<()> {
        if self.all_deps().any(|self_dep| self_dep == dep) {
            write_dependency_field_value(
                &self.manifest_path,
                &[dep],
                if remove_dep_path { &["path"] } else { &[] },
                "version",
                &version.to_string(),
                true,
            )?;
        }
        Ok(())
    }

    pub fn tweak_deps_for_publishing<P: AsRef<Path>>(&self, root: P) -> anyhow::Result<()> {
        /*
           Visit dev-dependencies and strip their `version` field before
           publishing. Reasoning: Since 1.40 (rust-lang/cargo#7333), cargo will
           strip dev-dependencies that don't have a version. This removes the
           need to manually strip dev-dependencies when publishing a crate that
           has circular dev-dependencies. (i.e., this works as a workaround of
           rust-lang/cargo#4242).
           Taken from https://github.com/rust-lang/futures-rs/pull/2305.
        */
        fn visit<P: AsRef<Path>>(
            dev_deps_tbl: &mut dyn toml_edit::TableLike,
            dev_deps_tbl_path: &str,
            dep: &str,
            toml_path: P,
        ) -> anyhow::Result<bool> {
            for (key, val) in dev_deps_tbl.iter_mut() {
                if key == dep {
                    let item = if val.as_str().is_some() {
                        continue;
                    } else {
                        val.as_table_like_mut().with_context(|| {
                            format!(
                                ".{}.{} should be a string or table-like in {:?}",
                                dev_deps_tbl_path,
                                key,
                                toml_path.as_ref().as_os_str()
                            )
                        })?
                    };
                    return Ok(item.get("path").is_some() && item.remove("version").is_some());
                } else {
                    let item = if val.as_str().is_some() {
                        continue;
                    } else {
                        val.as_table_like_mut().with_context(|| {
                            format!(
                                ".{}.{} should be a string or table-like in {:?}",
                                dev_deps_tbl_path,
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
                            return Ok(
                                item.get("path").is_some() && item.remove("version").is_some()
                            );
                        }
                    } else {
                        return Err(anyhow!(
                            ".{}.{}.package should be a string in {:?}",
                            dev_deps_tbl_path,
                            key,
                            toml_path.as_ref().as_os_str()
                        ));
                    }
                }
            }

            Ok(false)
        }

        let mut manifest = self.read_manifest()?;
        let mut needs_toml_write = false;

        let dev_deps_key = CrateDependencyKey::DevDependencies.to_string();

        // Visit [dev-dependencies]
        if let Some(item) = manifest.get_mut(&dev_deps_key) {
            let item = item.as_table_like_mut().with_context(|| {
                format!(
                    ".{} should be table-like in {:?}",
                    dev_deps_key,
                    self.manifest_path.as_os_str()
                )
            })?;
            for dev_dep in &self.dev_deps {
                if !self.deps_to_publish().any(|dep| dep == dev_dep) {
                    needs_toml_write |= visit(item, &dev_deps_key, dev_dep, &self.manifest_path)?;
                }
            }
        }

        // Visit [target.X.dev-dependencies]
        let targets_key = "target";
        if let Some(targets_tbl) = manifest.get_mut(targets_key) {
            let targets_tbl = targets_tbl.as_table_like_mut().with_context(|| {
                format!(
                    ".target should be table-like in {:?}",
                    self.manifest_path.as_os_str()
                )
            })?;
            for (target, target_tbl) in targets_tbl.iter_mut() {
                let target_path = format!("{}.{}", targets_key, target);
                let target_tbl = target_tbl.as_table_like_mut().with_context(|| {
                    format!(
                        ".{} should be table-like in {:?}",
                        target_path,
                        self.manifest_path.as_os_str()
                    )
                })?;
                if let Some(dev_deps_tbl) = target_tbl.get_mut(&dev_deps_key) {
                    let dev_deps_tbl_path = format!("{}.{}", target_path, dev_deps_key);
                    let dev_deps_tbl = dev_deps_tbl.as_table_like_mut().with_context(|| {
                        format!(
                            ".{} should be table-like in {:?}",
                            dev_deps_tbl_path,
                            self.manifest_path.as_os_str()
                        )
                    })?;
                    for dev_dep in &self.dev_deps {
                        if !self.deps_to_publish().any(|dep| dep == dev_dep) {
                            needs_toml_write |= visit(
                                dev_deps_tbl,
                                &dev_deps_tbl_path,
                                dev_dep,
                                &self.manifest_path,
                            )?;
                        }
                    }
                }
            }
        }

        if needs_toml_write {
            with_git_checkpoint(
                &root,
                GitCheckpoint::RevertLater,
                || -> anyhow::Result<()> { self.write_toml(&manifest) },
            )??;
        }

        Ok(())
    }

    pub fn tweak_readme_for_publishing<P: AsRef<Path>>(&self, root: P) -> anyhow::Result<()> {
        // In case a crate does NOT define a `readme` field in its `Cargo.toml`,
        // `cargo publish` assumes, without first checking, that a `README.md`
        // file exists beside `Cargo.toml`. Publishing will fail in case the
        // crate doesn't comply with that assumption. To work around that we'll
        // crate a sample `README.md` file for crates which don't specify or
        // have one.
        if self.readme.is_none() {
            let crate_readme = &self
                .manifest_path
                .parent()
                .with_context(|| format!("Failed to find parent dir of {:?}", &self.manifest_path))?
                .join("README.md");
            if fs::metadata(crate_readme).is_err() {
                with_git_checkpoint(
                    &root,
                    GitCheckpoint::RevertLater,
                    || -> anyhow::Result<()> {
                        fs::write(
                            crate_readme,
                            format!(
                                "# {}\n\nAuto-generated README.md for publishing to crates.io",
                                &self.name
                            ),
                        )
                        .with_context(|| {
                            format!("Failed to generate sample README at {:?}", crate_readme)
                        })
                    },
                )??;
            }
        }
        Ok(())
    }

    pub fn prepare_for_publish<P: AsRef<Path>>(&self, root: P) -> anyhow::Result<()> {
        self.tweak_deps_for_publishing(&root)?;
        self.tweak_readme_for_publishing(&root)?;
        Ok(())
    }

    pub fn publish(&self, verify: bool) -> Result<(), PublishError> {
        external::cargo::publish_crate(&self.name, &self.manifest_path, verify)
    }

    pub fn adjust_version(
        &mut self,
        prev_versions: &[CratesIoCrateVersion],
    ) -> anyhow::Result<bool> {
        let next_version = prev_versions
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
        if next_version != &self.version {
            self.write_own_version(next_version.to_owned())?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn needs_publishing<P: AsRef<Path>>(&self, root: P) -> anyhow::Result<bool> {
        info!(
            "Comparing crate {} against crates.io to see if it needs to be published",
            self.name
        );

        let span = span!(Level::INFO, "__", crate = self.name);
        let _enter = span.enter();

        enum TargetDir {
            Temp(TempDir),
            Path(PathBuf),
        }
        impl TargetDir {
            fn path(&self) -> PathBuf {
                match self {
                    TargetDir::Temp(tmp_dir) => tmp_dir.path().to_path_buf(),
                    TargetDir::Path(path) => path.clone(),
                }
            }
        }
        let target_dir = if let Ok(tmp_dir) = env::var("SPUB_TMP") {
            TargetDir::Path(PathBuf::from(tmp_dir))
        } else {
            TargetDir::Temp(tempfile::tempdir()?)
        };

        info!("Generating .crate file");
        self.prepare_for_publish(&root)?;
        let mut cmd = Command::new("cargo");
        if !cmd
            .arg("package")
            .arg("--manifest-path")
            .arg(&self.manifest_path)
            .arg("--no-verify")
            .arg("--allow-dirty")
            .arg("--target-dir")
            .arg(target_dir.path())
            .status()?
            .success()
        {
            return Err(anyhow!(
                "Failed to package crate {}. Command failed: {:?}",
                &self.name,
                cmd
            ));
        };
        git_checkpoint_revert(&root)?;

        let pkg_path = target_dir
            .path()
            .join("package")
            .join(format!("{}-{}.crate", &self.name, &self.version));
        let pkg_bytes = fs::read(&pkg_path)?;

        fn get_cratesio_bytes(
            name: &str,
            version: &semver::Version,
            #[allow(unused_variables)] pkg_bytes: &[u8],
        ) -> anyhow::Result<Option<Vec<u8>>> {
            #[cfg(test)]
            {
                external::crates_io::download_crate_for_testing(name, version, pkg_bytes)
            }
            #[cfg(not(test))]
            {
                external::crates_io::download_crate(name, version)
            }
        }
        let cratesio_bytes =
            if let Some(bytes) = get_cratesio_bytes(&self.name, &self.version, &pkg_bytes)? {
                bytes
            } else {
                info!("The crate is not published to crates.io");
                return Ok(true);
            };

        if cratesio_bytes == pkg_bytes {
            info!(
                "{:?} is identical to the version {} from crates.io",
                pkg_path, &self.version
            );
            Ok(false)
        } else {
            info!(
                "{:?} is different from the version {} from crates.io",
                pkg_path, &self.version
            );
            Ok(true)
        }
    }

    pub fn maybe_bump_version(
        &mut self,
        prev_versions: Vec<semver::Version>,
        bump_mode: &VersionBumpHeuristic,
    ) -> anyhow::Result<bool> {
        let new_version = match bump_mode {
            VersionBumpHeuristic::Breaking => {
                maybe_bump_for_breaking_change(prev_versions, self.version.clone())
            }
            VersionBumpHeuristic::Compatible => {
                maybe_bump_for_compatible_change(prev_versions, self.version.clone())
            }
        };
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

    fn read_manifest(&self) -> anyhow::Result<toml_edit::Document> {
        read_toml(&self.manifest_path)
    }

    fn write_toml(&self, toml: &toml_edit::Document) -> anyhow::Result<()> {
        write_toml(&self.manifest_path, toml)
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
fn filter_path_dependencies<P: AsRef<Path>>(
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
                    return Err(anyhow!(
                        "{} {} in {:?} should be specified as a string or table",
                        key_name,
                        key,
                        toml_path.as_ref(),
                    ));
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

#[cfg(all(test, any(feature = "test-1", feature = "test-2", feature = "test-3")))]
mod tests {
    use super::*;

    fn setup_details() -> (TempDir, CrateDetails) {
        let tmp_dir = tempfile::tempdir().unwrap();

        assert_eq!(
            {
                let mut cmd = Command::new("git");
                cmd.current_dir(&tmp_dir)
                    .arg("init")
                    .arg("--quiet")
                    .status()
                    .unwrap()
                    .success()
            },
            true
        );

        fs::write(
            tmp_dir.path().join("Cargo.toml"),
            r#"
[package]
name = "lib"
version = "0.1.0"
edition = "2021"
description = "placeholder"
license = "Apache-2.0"
documentation = "https://en.wikipedia.org/wiki/HTTP_404"
homepage = "https://en.wikipedia.org/wiki/HTTP_404"
repository = "https://en.wikipedia.org/wiki/HTTP_404"
            "#,
        )
        .unwrap();

        let src_dir = tmp_dir.path().join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::write(
            &src_dir.join("main.rs"),
            r#"
pub fn add(left: usize, right: usize) -> usize {
    left + right
}
            "#,
        )
        .unwrap();

        assert_eq!(
            {
                let mut cmd = Command::new("git");
                cmd.current_dir(&tmp_dir)
                    .arg("add")
                    .arg(".")
                    .status()
                    .unwrap()
                    .success()
            },
            true
        );
        assert_eq!(
            {
                let mut cmd = Command::new("git");
                cmd.current_dir(&tmp_dir)
                    .arg("commit")
                    .arg("--quiet")
                    .arg("--message")
                    .arg("initial commit")
                    .status()
                    .unwrap()
                    .success()
            },
            true
        );

        let details = CrateDetails::load(tmp_dir.path().join("Cargo.toml")).unwrap();

        (tmp_dir, details)
    }

    #[test]
    #[cfg(feature = "test-1")]
    pub fn test_crate_not_published_if_unchanged() {
        let (tmp_dir, details) = setup_details();
        assert_eq!(details.needs_publishing(&tmp_dir).unwrap(), false);
    }

    #[test]
    #[cfg(feature = "test-2")]
    pub fn test_crate_published_if_changed() {
        let (tmp_dir, details) = setup_details();
        assert_eq!(details.needs_publishing(&tmp_dir).unwrap(), true);
    }

    #[test]
    #[cfg(feature = "test-3")]
    pub fn test_crate_published_if_unpublished() {
        let (tmp_dir, details) = setup_details();
        assert_eq!(details.needs_publishing(&tmp_dir).unwrap(), true);
    }
}
