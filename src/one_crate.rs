use std::{path::PathBuf, io::Write};
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
    pub toml_path: PathBuf,
}

impl CrateDetails {
    /// Read a Cargo.toml file, pulling out the information we care about.
    pub fn load(path: PathBuf) -> anyhow::Result<CrateDetails> {
        let contents = std::fs::read(&path)?;
        let val: toml::Value = toml::from_slice(&contents)?;

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

        let deps = val
            .get("dependencies")
            .map(|deps| filter_workspace_dependencies(deps))
            .unwrap_or(Ok(HashSet::new()))?;

        let build_deps = val
            .get("build-dependencies")
            .map(|deps| filter_workspace_dependencies(deps))
            .unwrap_or(Ok(HashSet::new()))?;

        let dev_deps = val
            .get("dev-dependencies")
            .map(|deps| filter_workspace_dependencies(deps))
            .unwrap_or(Ok(HashSet::new()))?;

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

        if let Some(item) = toml.get_mut("dependencies") {
            do_set(item, version, dependency);
        }
        if let Some(item) = toml.get_mut("build-dependencies") {
            do_set(item, version, dependency);
        }
        if let Some(item) = toml.get_mut("dev-dependencies") {
            do_set(item, version, dependency);
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
    ///
    /// TODO: Implement this optimisation.
    pub fn needs_publishing(&self) -> anyhow::Result<bool> {
        use std::io::{ Cursor, Read };

        let name = &self.name;

        // Download and pass through a gzip decoder.
        let crate_bytes = crates_io::download_crate(&self.name, &self.version)
            .with_context(|| format!("Could not download crate {name}"))?;
        let crate_bytes = flate2::read::GzDecoder::new(Cursor::new(crate_bytes));

        // Iterate through the tar archive we decode.
        let mut archive = tar::Archive::new(crate_bytes);
        let entries = archive.entries()
            .with_context(|| format!("Could not read files in published crate {name}"))?;

        for entry in entries {
            let mut entry = entry
                .with_context(|| format!("Could not read files in published crate {name}"))?;
            let path = entry.path()
                .with_context(|| format!("Could not read path for crate {name}"))?
                .into_owned();

            let mut file_contents = vec![];
            entry.read_to_end(&mut file_contents)?;
// TODO log file contents to check we're decoding properly.
            println!("####################### FILE PATH: {:?}\n", path);
            std::io::stdout().write_all(&file_contents)?;
        }

        Ok(true)
    }

    /// Does this create need a version bump in order to be published?
    pub fn needs_version_bump_to_publish(&self) -> anyhow::Result<bool> {
        if self.version.pre != semver::Prerelease::EMPTY {
            // If prerelease eg `-dev`, we'll want to bump.
            return Ok(true)
        }

        // Does the current version of this crate exist on crates.io?
        let known_versions = crates_io::get_known_crate_versions(&self.name)?;
        Ok(!known_versions.contains(&self.version))
    }

    fn read_toml(&self) -> anyhow::Result<toml_edit::Document> {
        let name = &self.name;
        let toml_string = std::fs::read_to_string(&self.toml_path)
        .with_context(|| format!("Cannot read the Cargo.toml for {name}"))?;
        let toml = toml_string.parse::<toml_edit::Document>()
            .with_context(|| format!("Cannot parse the Cargo.toml for {name}"))?;
        Ok(toml)
    }

    fn write_toml(&self, toml: &toml_edit::Document) -> anyhow::Result<()> {
        let name = &self.name;
        std::fs::write(&self.toml_path, &toml.to_string())
            .with_context(|| format!("Cannot save the updated Cargo.toml for {name}"))?;
        Ok(())
    }
}

/// Given a path to some dependencies in a TOML file, pull out the package names
/// for any path based dependencies (ie dependencies in the same workspace).
fn filter_workspace_dependencies(val: &toml::Value) -> anyhow::Result<HashSet<String>> {
    let arr = match val.as_table() {
        Some(arr) => arr,
        None => return Err(anyhow!("dependencies should be a TOML table."))
    };

    let mut deps = HashSet::new();
    for (name, props) in arr {
        // If props arent a table eg { path = "/foo" }, this is
        // not a workspace dependency (since it needs a "path" prop)
        // so skip over it.
        let props = match props.as_table() {
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
            .unwrap_or(Ok(name.clone()))?;

        deps.insert(package_name);
    }

    Ok(deps)
}
