use std::path::PathBuf;
use anyhow::{anyhow, Context};
use semver::Version;
use std::collections::HashSet;

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
    pub fn bump_version(&mut self) -> anyhow::Result<Version> {
        let name = &self.name;
        let mut new_version = self.version.clone();

        if new_version.pre != semver::Prerelease::EMPTY {
            // Remove pre-release tag like `-dev` if present
            new_version.pre = semver::Prerelease::EMPTY;
        } else if new_version.major == 0 {
            // Else, bump minor if 0.x.0 crate
            new_version.minor += 1;
        } else {
            // Else bump major version
            new_version.major += 1;
        }

        // Load TOML file and update the version in that.
        let mut toml = self.read_toml()?;
        toml["version"] = toml_edit::value(new_version.to_string());
        self.write_toml(&toml)?;

        // If that worked, save the in-memory version too
        self.version = new_version.clone();

        Ok(new_version)
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
                if let Some(version) = table.get_mut("version") {
                    *version = toml_edit::value(version.to_string());
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
        let name = &self.name;

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
        Ok(true)
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
