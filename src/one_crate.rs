use std::path::PathBuf;
use anyhow::anyhow;

#[derive(Debug, Clone)]
pub struct CrateDetails {
    pub name: String,
    pub version: String,
    pub deps: Vec<String>,
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
            .ok_or_else(|| anyhow!("Cannot read [package] section from toml file."))?
            .get("version")
            .ok_or_else(|| anyhow!("Cannot read package.version from toml file."))?
            .as_str()
            .ok_or_else(|| anyhow!("Cannot read package.version from toml file."))?
            .to_owned();

        let deps = {
            let mut deps = val
                .get("dependencies")
                .map(|deps| filter_workspace_dependencies(deps))
                .unwrap_or(Ok(vec![]))?;
            let build_deps = val
                .get("build-dependencies")
                .map(|deps| filter_workspace_dependencies(deps))
                .unwrap_or(Ok(vec![]))?;
            deps.extend(build_deps);
            deps
        };

        Ok(CrateDetails {
            name,
            version,
            deps,
            toml_path: path,
        })
    }
}

/// Given a path to some dependencies in a TOML file, pull out the package names
/// for any path based dependencies (ie dependencies in the same workspace).
fn filter_workspace_dependencies(val: &toml::Value) -> anyhow::Result<Vec<String>> {
    let arr = match val.as_table() {
        Some(arr) => arr,
        None => return Err(anyhow!("dependencies should be a TOML table."))
    };

    let mut deps = vec![];
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

        deps.push(package_name);
    }

    Ok(deps)
}