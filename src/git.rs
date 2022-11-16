use std::path::Path;
use std::process::Command;

const CHECKPOINT_SAVE: &'static str = "[subpub] CHECKPOINT_SAVE";
const CHECKPOINT_REVERT: &'static str = "[subpub] CHECKPOINT_REVERT";

pub enum GitCheckpoint {
    Save,
    Revert,
}

pub fn git_checkpoint<P>(root: P, op: GitCheckpoint) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    let mut cmd = Command::new("git");
    let git_status_output = cmd
        .current_dir(root.as_ref())
        .arg("status")
        .arg("--porcelain")
        .arg("v1")
        .output()?;
    if git_status_output.stdout.is_empty() {
        let mut cmd = Command::new("git");
        if cmd
            .current_dir(root.as_ref())
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

    let mut cmd = Command::new("git");
    if cmd
        .current_dir(root.as_ref())
        .arg("add")
        .arg("--quiet")
        .arg(".")
        .status()?
        .success()
    {
        let mut cmd = Command::new("git");
        cmd.current_dir(root)
            .arg("commit")
            .arg("-m")
            .arg(match op {
                GitCheckpoint::Save => CHECKPOINT_SAVE,
                GitCheckpoint::Revert => CHECKPOINT_REVERT,
            })
            .status()?;
        Ok(())
    } else {
        anyhow::bail!("Unable to commit modified files:\n{:?}", git_status_output);
    }
}
