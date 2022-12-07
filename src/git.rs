use std::{path::Path, process::Command};

const CHECKPOINT_SAVE: &str = "[subpub] CHECKPOINT_SAVE";
const CHECKPOINT_REVERT: &str = "[subpub] CHECKPOINT_REVERT";

pub enum GitCheckpoint {
    Save,
    RevertLater,
}

fn git_checkpoint<P: AsRef<Path>>(root: P, op: GitCheckpoint) -> anyhow::Result<()> {
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
            GitCheckpoint::Save => CHECKPOINT_SAVE,
            GitCheckpoint::RevertLater => CHECKPOINT_REVERT,
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

pub fn with_git_checkpoint<T, P: AsRef<Path>, F: FnOnce() -> T>(
    root: P,
    op: GitCheckpoint,
    func: F,
) -> anyhow::Result<T> {
    git_checkpoint(&root, GitCheckpoint::Save)?;
    let result = func();
    git_checkpoint(&root, op)?;
    Ok(result)
}

pub fn git_checkpoint_revert<P: AsRef<Path>>(root: P) -> anyhow::Result<()> {
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
