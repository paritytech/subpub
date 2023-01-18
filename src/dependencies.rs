use std::path::Path;

use anyhow::{anyhow, Context};
use strum::{EnumIter, EnumString, IntoEnumIterator};

use crate::toml::{read_toml, write_toml};

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
                            ".{}.{} should be a string or table-like in {:?}",
                            dep_key_display,
                            key,
                            toml_path.as_ref().as_os_str()
                        )
                    })?;
                    if item.get("workspace").is_some() {
                        return Err(
                            anyhow!(
                                ".workspace is not supported for dependencies, but it's used for .{}.{} in {:?}",
                                dep_key_display,
                                key,
                                toml_path.as_ref().as_os_str()
                            )
                        );
                    }
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
                            ".{}.{} should be a string or table-like in {:?}",
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
                        if item.get("workspace").is_some() {
                            return Err(
                                 anyhow!(
                                    ".workspace is not supported for dependencies, but it's used for .{}.{} in {:?}",
                                    dep_key_display,
                                    key,
                                    toml_path.as_ref().as_os_str()
                                )
                            );
                        }
                        item.insert("version", toml_edit::value(version.to_string()));
                        if remove_dependency_path {
                            item.remove("path");
                        }
                    }
                } else {
                    return Err(anyhow!(
                        "{}.{}.package should be a string in {:?}",
                        dep_key_display,
                        key,
                        toml_path.as_ref().as_os_str()
                    ));
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
