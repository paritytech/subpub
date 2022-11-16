use anyhow::Context;
use std::path::Path;
use std::process::Command;

const CHECKPOINT_SAVE: &'static str = "[subpub] CHECKPOINT_SAVE";
const CHECKPOINT_REVERT: &'static str = "[subpub] CHECKPOINT_REVERT";

const THIS_FILE: &'static str = file!();

pub enum GCM {
    Save,
    RevertLater,
}

pub fn git_checkpoint<P>(root: P, op: GCM) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    let mut cmd = Command::new("git");
    let git_status_output = cmd
        .current_dir(&root)
        .arg("status")
        .arg("--porcelain=v1")
        .output()?;
    if !git_status_output.status.success() {
        anyhow::bail!(
            "Failed to get git status for {:?}",
            root.as_ref().as_os_str()
        );
    }

    let git_status_output = String::from_utf8_lossy(&git_status_output.stdout[..]);
    let git_status_output = git_status_output.trim();
    if !git_status_output.is_empty() {
        let mut cmd = Command::new("git");
        if !cmd
            .current_dir(&root)
            .arg("add")
            .arg(".")
            .status()?
            .success()
        {
            anyhow::bail!(
                "Failed to `git add` files for {:?}",
                root.as_ref().as_os_str()
            );
        }

        let commit_msg = match op {
            GCM::Save => CHECKPOINT_SAVE,
            GCM::RevertLater => CHECKPOINT_REVERT,
        };
        let mut cmd = Command::new("git");
        if !cmd
            .current_dir(&root)
            .arg("commit")
            .arg("--quiet")
            .arg("-m")
            .arg(commit_msg)
            .status()?
            .success()
        {
            anyhow::bail!(
                "Failed to `git commit` files for {:?}",
                root.as_ref().as_os_str()
            );
        }
    };

    Ok(())
}

pub fn git_checkpoint_revert<P>(root: P) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    loop {
        let mut cmd = Command::new("git");
        let output = cmd
            .current_dir(&root)
            .arg("log")
            .arg("-1")
            .arg("--pretty=%B")
            .output()?;
        if !output.status.success() {
            anyhow::bail!("Failed to get commit message of last commit");
        }

        let last_commit_msg = String::from_utf8_lossy(&output.stdout[..]);
        let last_commit_msg = last_commit_msg.trim();
        if last_commit_msg == CHECKPOINT_REVERT {
            let mut cmd = Command::new("git");
            if !cmd
                .current_dir(&root)
                .arg("reset")
                .arg("--quiet")
                .arg("--hard")
                .arg("HEAD~1")
                .status()?
                .success()
            {
                anyhow::bail!("Failed to revert checkpoint commit");
            }
        } else {
            break;
        }
    }
    Ok(())
}

pub fn git_checkpoint_revert_all<P>(root: P) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    let mut cmd = Command::new("git");
    let output = cmd
        .current_dir(&root)
        .arg("log")
        .arg("--pretty=%H\0%B\0")
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to get commit message of last commit");
    }

    let output = String::from_utf8_lossy(&output.stdout[..]);
    let mut output = output.split("\0");

    let mut drop = vec![];
    let mut keep = vec![];
    let mut last_commit = None;
    while let Some(commit_sha) = output.next() {
        if commit_sha.is_empty() {
            break;
        }
        let commit_msg = output
            .next()
            .with_context(|| format!("Expected commit message for commit {commit_sha}"))?;
        match commit_msg {
            CHECKPOINT_REVERT => {
                drop.push(commit_sha);
                last_commit = Some(commit_sha);
            }
            CHECKPOINT_SAVE => {
                last_commit = Some(commit_sha);
                keep.push(commit_sha);
            }
            _ => break,
        }
    }

    let last_commit = if let Some(commit) = last_commit {
        commit
    } else {
        return Ok(());
    };

    let script_filter_path = Path::new(THIS_FILE.into())
        .parent()
        .with_context(|| format!("Failed to parse parent of {THIS_FILE}"))?
        .join("commit-filter.sh");
    let mut cmd = Command::new("git");
    if !cmd
        .current_dir(&root)
        .arg("rebase")
        .arg("-i")
        .arg("-c")
        .arg(format!("sequence.editor={}", script_filter_path.display()))
        .arg(last_commit)
        .status()?
        .success()
    {
        anyhow::bail!("Failed to filter commit list");
    };

    Ok(())
}
