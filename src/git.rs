use std::path::Path;
use std::process::Command;

const CHECKPOINT_SAVE: &'static str = "[subpub] CHECKPOINT_SAVE";
const CHECKPOINT_REVERT: &'static str = "[subpub] CHECKPOINT_REVERT";

pub enum GitCheckpointMode {
    Save,
    RevertLater,
}

pub fn git_checkpoint<P>(root: P, op: GitCheckpointMode) -> anyhow::Result<()>
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
            "Unable to get git status for {:?}",
            root.as_ref().as_os_str()
        );
    }

    let mut created_revert_commit = false;

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
            anyhow::bail!("Unable to commit modified files:\n{:?}", git_status_output);
        }

        let mut cmd = Command::new("git");
        if !cmd
            .current_dir(&root)
            .arg("commit")
            .arg("--quiet")
            .arg("-m")
            .arg(match op {
                GitCheckpointMode::Save => CHECKPOINT_SAVE,
                GitCheckpointMode::RevertLater => {
                    created_revert_commit = true;
                    CHECKPOINT_REVERT
                }
            })
            .status()?
            .success()
        {
            anyhow::bail!("Unable to commit modified files:\n{:?}", git_status_output);
        }
    }

    if !created_revert_commit {
        let mut cmd = Command::new("git");
        if !cmd
            .current_dir(&root)
            .arg("commit")
            .arg("--quiet")
            .arg("--allow-empty")
            .arg("-m")
            .arg(CHECKPOINT_REVERT)
            .status()?
            .success()
        {
            anyhow::bail!("Unable to create empty commit");
        }
    }

    Ok(())
}

pub fn git_revert<P>(root: P) -> anyhow::Result<()>
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
            anyhow::bail!("Unable to get commit message of last commit");
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
                anyhow::bail!("Unable to revert checkpoint commit");
            }
        } else {
            break;
        }
    }
    Ok(())
}
