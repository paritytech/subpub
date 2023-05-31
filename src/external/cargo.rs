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

use std::path::Path;
use std::process::Command;

/// Update the lockfile for dependencies given and any of their subdependencies.
pub fn update_lockfile_for_crates<I, S>(root: &Path, deps: I) -> anyhow::Result<()>
where
    S: AsRef<str>,
    I: IntoIterator<Item = S>,
{
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root).arg("update");

    for dep in deps.into_iter() {
        cmd.arg("-p").arg(dep.as_ref());
    }

    cmd.status()?;
    Ok(())
}

/// Update the lockfile for dependencies given and any of their subdependencies.
pub fn publish_crate(root: &Path, package: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("cargo");

    cmd.current_dir(root)
        .arg("publish")
        .arg("-p")
        .arg(package)
        .arg("--allow-dirty")
        .status()?;

    Ok(())
}
