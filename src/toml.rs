use std::{
    fs::{self, read_to_string},
    path::Path,
};

use anyhow::Context;

pub fn read_toml<P: AsRef<Path>>(path: P) -> anyhow::Result<toml_edit::Document> {
    let toml_string = read_to_string(&path).with_context(|| {
        format!(
            "Cannot read the Cargo.toml at {:?}",
            path.as_ref().as_os_str()
        )
    })?;
    let toml = toml_string
        .parse::<toml_edit::Document>()
        .with_context(|| {
            format!(
                "Cannot parse the Cargo.toml at {:?}",
                path.as_ref().as_os_str()
            )
        })?;
    Ok(toml)
}

pub fn write_toml<P: AsRef<Path>>(path: P, toml: &toml_edit::Document) -> anyhow::Result<()> {
    let mut content = toml.to_string();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    fs::write(&path, content).with_context(|| {
        format!(
            "Cannot save the updated Cargo.toml at {:?}",
            path.as_ref().as_os_str()
        )
    })?;
    Ok(())
}
