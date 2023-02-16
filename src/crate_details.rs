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
use cargo_metadata::{DependencyKind, Package};
use external::crates_io::CratesIoCrateVersion;
use semver::Version;
use tempfile::TempDir;
use tracing::{info, span, Level};

use crate::{
    dependencies::{write_dependency_field, DependencyFieldType, ManifestDependencyKey},
    external::{self, cargo::PublishError},
    testing::TestEnvironment,
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
    pub description: Option<String>,
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
            manifest_path: PathBuf::new(),
            readme: None,
            description: Some("Placeholder description".into()),
        }
    }

    pub fn load(pkg: &Package) -> anyhow::Result<CrateDetails> {
        let path_deps = pkg.dependencies.iter().filter(|dep| dep.path.is_some());

        let deps = HashSet::from_iter(path_deps.clone().filter_map(|dep| {
            if dep.kind == DependencyKind::Normal {
                Some(dep.name.clone())
            } else {
                None
            }
        }));

        let dev_deps = HashSet::from_iter(path_deps.clone().filter_map(|dep| {
            if dep.kind == DependencyKind::Development {
                Some(dep.name.clone())
            } else {
                None
            }
        }));

        let build_deps = HashSet::from_iter(path_deps.clone().filter_map(|dep| {
            if dep.kind == DependencyKind::Build {
                Some(dep.name.clone())
            } else {
                None
            }
        }));

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
            description: pkg.description.clone(),
        })
    }

    pub fn write_own_version(&mut self, new_version: Version) -> anyhow::Result<()> {
        let mut manifest = self.read_manifest()?;
        manifest["package"]["version"] = toml_edit::value(new_version.to_string());
        self.write_toml(&manifest)?;

        self.version = new_version;

        Ok(())
    }

    pub fn deps_to_publish(&self) -> impl Iterator<Item = &String> {
        self.deps.iter()
    }

    pub fn write_dependency_version<P: AsRef<Path>>(
        &self,
        root: P,
        dep: &str,
        version: &Version,
        fields_to_remove: &[&str],
    ) -> anyhow::Result<()> {
        for manifest_path in &[&root.as_ref().join("Cargo.toml"), &self.manifest_path] {
            write_dependency_field(
                manifest_path,
                &[dep],
                fields_to_remove,
                "version",
                &version.to_string(),
                DependencyFieldType::Version,
            )?;
        }
        Ok(())
    }

    pub fn tweak_deps_for_publishing(&self) -> anyhow::Result<()> {
        /*
           Visit dev-dependencies and strip their `version` field before
           publishing. Reasoning: Since 1.40 (rust-lang/cargo#7333), cargo will
           strip dev-dependencies that don't have a version. This removes the
           need to manually strip dev-dependencies when publishing a crate that
           has circular dev-dependencies (i.e. this is a workaround for
           https://github.com/rust-lang/cargo/issues/4242).
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

        let dev_deps_key = ManifestDependencyKey::DevDependencies.to_string();

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
            self.write_toml(&manifest)?;
        }

        Ok(())
    }

    pub fn tweak_readme_for_publishing(&self) -> anyhow::Result<()> {
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
                fs::write(
                    crate_readme,
                    format!(
                        "# {}\n\nAuto-generated README.md for publishing to crates.io",
                        &self.name
                    ),
                )?;
            }
        }
        Ok(())
    }

    pub fn tweak_description_for_publishing(&self) -> anyhow::Result<()> {
        let mut manifest = read_toml(&self.manifest_path)?;
        if self.description.is_none() {
            manifest["package"]["description"] = toml_edit::value(&self.name);
            self.write_toml(&manifest)?;
        }
        Ok(())
    }

    pub fn prepare_for_publish(&self) -> anyhow::Result<()> {
        self.tweak_deps_for_publishing()?;
        self.tweak_readme_for_publishing()?;
        self.tweak_description_for_publishing()?;
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

    pub fn needs_publishing(&self, test_env: Option<TestEnvironment>) -> anyhow::Result<bool> {
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

        self.prepare_for_publish()?;

        let mut cmd = Command::new("cargo");
        cmd.arg("package")
            .arg("--quiet")
            .arg("--manifest-path")
            .arg(&self.manifest_path)
            .arg("--no-verify")
            .arg("--allow-dirty")
            .arg("--target-dir")
            .arg(target_dir.path());
        if test_env.is_some() {
            cmd.arg("--quiet");
        }

        if !cmd.status()?.success() {
            return Err(anyhow!(
                "Failed to package crate {}. Command failed: {:?}",
                &self.name,
                cmd
            ));
        };

        let pkg_path = target_dir
            .path()
            .join("package")
            .join(format!("{}-{}.crate", &self.name, &self.version));
        let pkg_bytes = fs::read(&pkg_path)?;

        fn get_cratesio_bytes(
            name: &str,
            version: &semver::Version,
            pkg_bytes: &[u8],
            test_env: Option<TestEnvironment>,
        ) -> anyhow::Result<Option<Vec<u8>>> {
            if let Some(test_env) = test_env {
                external::crates_io::download_crate_for_testing(name, version, pkg_bytes, test_env)
            } else {
                external::crates_io::download_crate(name, version)
            }
        }
        let cratesio_bytes = if let Some(bytes) =
            get_cratesio_bytes(&self.name, &self.version, &pkg_bytes, test_env)?
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_details() -> (TempDir, CrateDetails) {
        let project_dir = tempfile::tempdir().unwrap();

        assert!({
            let mut cmd = Command::new("git");
            cmd.current_dir(&project_dir)
                .arg("init")
                .arg("--quiet")
                .status()
                .unwrap()
                .success()
        });

        fs::write(
            project_dir.path().join("Cargo.toml"),
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

        let src_dir = project_dir.path().join("src");
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

        assert!({
            let mut cmd = Command::new("git");
            cmd.current_dir(&project_dir)
                .arg("add")
                .arg(".")
                .status()
                .unwrap()
                .success()
        });

        assert!({
            let mut cmd = Command::new("git");
            cmd.current_dir(&project_dir)
                .arg("commit")
                .arg("--quiet")
                .arg("--message")
                .arg("initial commit")
                .status()
                .unwrap()
                .success()
        });

        let workspace_meta = cargo_metadata::MetadataCommand::new()
            .current_dir(project_dir.path())
            .exec()
            .unwrap();

        let pkg = workspace_meta
            .packages
            .iter()
            .find(|pkg| pkg.name == "lib")
            .unwrap();

        let details = CrateDetails::load(pkg).unwrap();

        (project_dir, details)
    }

    #[test]
    pub fn test_crate_not_published_if_unchanged() {
        let (_tmp_dir, details) = setup_details();
        assert!(!details
            .needs_publishing(Some(TestEnvironment::CrateNotPublishedIfUnchanged))
            .unwrap(),);
    }

    #[test]
    pub fn test_crate_published_if_changed() {
        let (_tmp_dir, details) = setup_details();
        assert!(details
            .needs_publishing(Some(TestEnvironment::CratePublishedIfChanged))
            .unwrap(),);
    }

    #[test]
    pub fn test_crate_published_if_not_published() {
        let (_tmp_dir, details) = setup_details();
        assert!(details
            .needs_publishing(Some(TestEnvironment::CratePublishedIfNotPublished))
            .unwrap(),);
    }
}
