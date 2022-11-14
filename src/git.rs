use std::path::PathBuf;
use std::process::Command;

pub fn git_checkpoint(root: &PathBuf, msg: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("git");
    let git_status_output = cmd
        .current_dir(root)
        .arg("status")
        .arg("--porcelain")
        .arg("v1")
        .output()?;
    if git_status_output.stdout.is_empty() {
        return Ok(());
    }

    let mut cmd = Command::new("git");
    if cmd
        .current_dir(root)
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
            .arg(msg)
            .status()?;
        Ok(())
    } else {
        anyhow::bail!("Unable to commit modified files:\n{:?}", git_status_output);
    }
}
