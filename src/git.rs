use anyhow::Context;
use std::fs;
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
        .arg("--pretty=%H\n%B")
        .output()?;
    if !output.status.success() {
        anyhow::bail!("Failed to get commit message of last commit");
    }

    let output = String::from_utf8_lossy(&output.stdout[..]);
    let mut output = output.split("\n");

    let (rebase_cmds, last_commit) = {
        let mut rebase_cmds = String::new();
        let mut last_commit = None;
        while let Some(commit_sha) = output.next() {
            if commit_sha.is_empty() {
                continue;
            }
            let commit_msg = output
                .next()
                .with_context(|| format!("Expected commit message for commit {commit_sha}"))?;
            let commit_msg = commit_msg.trim();
            match commit_msg {
                CHECKPOINT_REVERT => {
                    rebase_cmds.push_str(&format!("drop {commit_sha}\n"));
                    last_commit = Some(commit_sha);
                }
                CHECKPOINT_SAVE => {
                    rebase_cmds.push_str(&format!("pick {commit_sha}\n"));
                    last_commit = Some(commit_sha);
                }
                _ => break,
            }
        }
        (rebase_cmds, last_commit)
    };

    let last_commit = if let Some(commit) = last_commit {
        commit
    } else {
        return Ok(());
    };

    let filter_script_path = Path::new(THIS_FILE.into())
        .parent()
        .with_context(|| format!("Failed to parse the parent of {THIS_FILE}"))?
        .parent()
        .with_context(|| format!("Failed to parse the parent's parent of {THIS_FILE}"))?
        .join("commit-filter.sh");
    let filter_script_path = if filter_script_path.is_absolute() {
        filter_script_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(filter_script_path)
    };

    let tmp_dir = tempfile::tempdir()?;
    let rebase_cmds_path = &tmp_dir.path().join("rebase-commands");
    fs::write(rebase_cmds_path, rebase_cmds)?;

    let mut cmd = Command::new("git");
    if !cmd
        .current_dir(&root)
        .env("REBASE_COMMANDS", rebase_cmds_path.as_os_str())
        .arg("-c")
        .arg(format!("sequence.editor={}", filter_script_path.display()))
        .arg("rebase")
        .arg("-i")
        .arg(format!("{}~1", last_commit))
        .status()?
        .success()
    {
        anyhow::bail!("Failed to filter commit list");
    };

    Ok(())
}
