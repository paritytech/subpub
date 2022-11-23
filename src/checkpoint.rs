use crate::git::{git_checkpoint, GCKP};
use std::path::Path;

pub fn with_save_checkpoint<T, P: AsRef<Path>, F: FnOnce() -> T>(
    root: P,
    func: F,
) -> anyhow::Result<T> {
    git_checkpoint(&root, GCKP::Save)?;
    let result = func();
    git_checkpoint(&root, GCKP::Save)?;
    Ok(result)
}
