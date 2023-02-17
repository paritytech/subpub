use std::{fs, path::Path};

use anyhow::Context;

pub fn read_toml<P: AsRef<Path>>(path: P) -> anyhow::Result<toml_edit::Document> {
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read file {:?}", path.as_ref().as_os_str()))?;
    let toml = content.parse::<toml_edit::Document>().with_context(|| {
        format!(
            "Failed to parse file as TOML: {:?}",
            path.as_ref().as_os_str()
        )
    })?;
    Ok(toml)
}

pub fn write_toml<P: AsRef<Path>>(path: P, doc: &toml_edit::Document) -> anyhow::Result<()> {
    let mut content = doc.to_string();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    fs::write(&path, content).with_context(|| {
        format!(
            "Failed to write contents to {:?}",
            path.as_ref().as_os_str()
        )
    })?;
    Ok(())
}
