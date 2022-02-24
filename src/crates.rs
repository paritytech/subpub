use std::path::PathBuf;
use walkdir::WalkDir;
use anyhow::anyhow;
use std::collections::{ HashMap, HashSet };
use crate::one_crate::CrateDetails;

#[derive(Debug, Clone)]
pub struct Crates {
    // Details for a given crate, including dependencies.
    details: HashMap<String, CrateDetails>,
    // Which crates depend on a given crate.
    dependees: HashMap<String, HashSet<String>>
}

impl Crates {
    /// Return a map of all substrate crates, in the form `crate_name => ( path, details )`.
    pub fn load_crates_in_workspace(root: PathBuf) -> anyhow::Result<Crates> {
        // Load details:
        let details = crate_cargo_tomls(root)
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
        let mut dependees: HashMap<String, HashSet<String>> = HashMap::new();
        for crate_details in details.values() {
            for dep in &crate_details.deps {
                dependees.entry(dep.clone()).or_default().insert(crate_details.name.clone());
            }
        }

        Ok(Crates {
            details,
            dependees,
        })
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
        // Ignore hidden files and folders.
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|s| !s.starts_with("."))
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

