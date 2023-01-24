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
    dep_key: &CrateDependencyKey,
    mut f: F,
) -> anyhow::Result<()> {
    let dep_key_display = dep_key.to_string();
    if let Some(item) = document.get_mut(&dep_key_display) {
        f(item, dep_key, &dep_key_display)?;
    }
    for item in get_target_dependency_sections_mut(document, &dep_key_display) {
        f(item, dep_key, &dep_key_display)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn write_dependency_field_value<P: AsRef<Path>, S: AsRef<str>>(
    manifest_path: P,
    deps: &[S],
    fields_to_remove: &[&str],
    field: &str,
    field_value: &str,
    overwrite_str_value: bool,
) -> anyhow::Result<()> {
    let mut manifest = read_toml(&manifest_path)?;

    fn visit<P: AsRef<Path>, S: AsRef<str>>(
        item: &mut toml_edit::Item,
        deps: &[S],
        dep_key_display: &str,
        manifest_path: P,
        fields_to_remove: &[&str],
        field: &str,
        field_value: &str,
        overwrite_str_value: bool,
    ) -> anyhow::Result<bool> {
        let deps_tbl = item.as_table_like_mut().with_context(|| {
            format!(
                ".{} should be table-like in {:?}",
                dep_key_display,
                manifest_path.as_ref().as_os_str()
            )
        })?;

        fn edit_tablelike_dep<P: AsRef<Path>>(
            key: &toml_edit::KeyMut,
            value: &mut dyn toml_edit::TableLike,
            dep_key_display: &str,
            manifest_path: P,
            field: &str,
            field_value: &str,
            fields_to_remove: &[&str],
        ) -> anyhow::Result<()> {
            if value.get("workspace").is_some() {
                return Err(anyhow!(
                    ".{}.{}.workspace is not supported in {:?}",
                    dep_key_display,
                    key,
                    manifest_path.as_ref().as_os_str()
                ));
            }
            value.insert(field, toml_edit::value(field_value));
            for fields_to_remove in fields_to_remove {
                value.remove(fields_to_remove);
            }
            Ok(())
        }

        let mut modified = false;

        for (key, value) in deps_tbl.iter_mut() {
            if let Some(value) = value.as_table_like_mut() {
                if let Some(pkg) = value.get("package") {
                    let pkg = pkg.as_str().with_context(|| {
                        format!(
                            ".{}.{}.package should be a string in {:?}",
                            dep_key_display,
                            key,
                            manifest_path.as_ref().as_os_str()
                        )
                    })?;
                    if deps.iter().any(|dep| pkg == dep.as_ref()) {
                        edit_tablelike_dep(
                            &key,
                            value,
                            dep_key_display,
                            &manifest_path,
                            field,
                            field_value,
                            fields_to_remove,
                        )?;
                        modified = true;
                    }
                } else if deps.iter().any(|dep| dep.as_ref() == key.get()) {
                    edit_tablelike_dep(
                        &key,
                        value,
                        dep_key_display,
                        &manifest_path,
                        field,
                        field_value,
                        fields_to_remove,
                    )?;
                    modified = true;
                }
            } else if let Some(version) = value.as_str() {
                if deps.iter().any(|dep| dep.as_ref() == key.get()) {
                    if overwrite_str_value {
                        *value = toml_edit::value(field_value);
                    } else {
                        let mut tbl = toml_edit::InlineTable::new();
                        tbl.insert("version", version.into());
                        tbl.insert(field, field_value.into());
                        *value = toml_edit::Item::Value(toml_edit::Value::InlineTable(tbl));
                    }
                    modified = true;
                }
            } else {
                return Err(anyhow!(
                    ".{}.{} should be a string or table-like in {:?}",
                    dep_key_display,
                    key,
                    manifest_path.as_ref().as_os_str()
                ));
            }
        }

        Ok(modified)
    }

    let mut modified = false;

    for dep_key in CrateDependencyKey::iter() {
        edit_all_dependency_sections(&mut manifest, &dep_key, |item, _, dep_key_display| {
            modified |= visit(
                item,
                deps,
                dep_key_display,
                &manifest_path,
                fields_to_remove,
                field,
                field_value,
                overwrite_str_value,
            )?;
            Ok(())
        })?;
    }

    if modified {
        write_toml(manifest_path, &manifest)?;
    }

    Ok(())
}
