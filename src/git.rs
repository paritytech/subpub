use std::{fs, path::Path, process::Command};

use anyhow::anyhow;

pub fn git_head_sha<P: AsRef<Path>>(root: P) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    let output = cmd
        .current_dir(&root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "Failed to get the HEAD sha of {:?}. Command failed: {:?}",
            root.as_ref(),
            cmd
        ));
    }
    let head_sha = String::from_utf8_lossy(&output.stdout[..])
        .trim()
        .to_string();
    Ok(head_sha)
}

pub fn git_hard_reset<P: AsRef<Path>>(root: P, initial_commit: &str) -> anyhow::Result<()> {
    // A left-over index.lock file might block us from resetting to the
    // initial_commit, so handle it prior to attempting the reset.
    let git_index_lock_path = root.as_ref().join(".git").join("index.lock");
    if fs::metadata(&git_index_lock_path).is_ok() {
        fs::remove_file(&git_index_lock_path)?;
    }

    let mut cmd = Command::new("git");
    if !cmd
        .current_dir(&root)
        .arg("add")
        .arg(".")
        .status()?
        .success()
    {
        return Err(anyhow!(
            "Failed to run `git add` in {:?}. Command failed: {:?}",
            root.as_ref(),
            cmd
        ));
    }

    let mut cmd = Command::new("git");
    if !cmd
        .current_dir(&root)
        .arg("reset")
        .arg("--quiet")
        .arg("--hard")
        .arg(initial_commit)
        .status()?
        .success()
    {
        return Err(anyhow!(
            "Failed to `git reset` the files of {:?}. Command failed: {:?}",
            root.as_ref(),
            cmd
        ));
    }

    Ok(())
}

pub fn git_remote_head_sha<S: AsRef<str>>(remote: S) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    let output = cmd.arg("ls-remote").arg(remote.as_ref()).output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "Failed to query the remote HEAD sha of {}. Command failed: {:?}",
            remote.as_ref(),
            cmd
        ));
    }
    let output = String::from_utf8_lossy(&output.stdout[..])
        .trim()
        .to_string();
    for line in output.lines() {
        let line = line.trim();
        if line.ends_with("HEAD") {
            let mut parts = line.split_whitespace();
            if let Some(head_sha) = parts.next() {
                return Ok(head_sha.to_string());
            }
        }
    }
    Err(anyhow!(
        "Failed to parse HEAD sha of {} from the output of {:?}\nOutput:\n{}",
        remote.as_ref(),
        cmd,
        output
    ))
}
