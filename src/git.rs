use std::path::Path;
use std::process::Command;

const CHECKPOINT_SAVE: &'static str = "[subpub] CHECKPOINT_SAVE";
const CHECKPOINT_REVERT: &'static str = "[subpub] CHECKPOINT_REVERT";

pub fn git_checkpoint<P>(root: P) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    let mut cmd = Command::new("git");
    let git_status_output = cmd
        .current_dir(&root)
        .arg("status")
        .arg("--porcelain")
        .arg("v1")
        .output()?;

    if !git_status_output.stdout.is_empty() {
        let mut cmd = Command::new("git");
        if !cmd
            .current_dir(&root)
            .arg("add")
            .arg("--quiet")
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
            .arg("-m")
            .arg(CHECKPOINT_SAVE)
            .status()?
            .success()
        {
            anyhow::bail!("Unable to commit modified files:\n{:?}", git_status_output);
        }
    }

    let mut cmd = Command::new("git");
    if cmd
        .current_dir(&root)
        .arg("commit")
        .arg("--allow-empty")
        .arg("-m")
        .arg(CHECKPOINT_REVERT)
        .status()?
        .success()
    {
        return Ok(());
    } else {
        anyhow::bail!("Unable to create empty commit");
    }
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
        if last_commit_msg == CHECKPOINT_REVERT {
            let mut cmd = Command::new("git");
            if !cmd
                .current_dir(&root)
                .arg("reset")
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
