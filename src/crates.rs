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

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context};
use tracing::info;

use crate::{
    crate_details::CrateDetails,
    external::{self, cargo::PublishError, crates_io::CratesIoIndexConfiguration},
};

pub type CrateName = String;
#[derive(Debug, Clone)]
pub struct Crates {
    pub root: PathBuf,
    pub crates_map: HashMap<CrateName, CrateDetails>,
}

impl Crates {
    pub fn load_workspace_crates(root: PathBuf) -> anyhow::Result<Crates> {
        let workspace_meta = cargo_metadata::MetadataCommand::new()
            .current_dir(&root)
            .exec()
            .with_context(|| format!("Failed to run cargo_metadata for {:?}", &root))?;

        let crates_map = {
            let mut crates_map: HashMap<String, CrateDetails> = HashMap::new();
            for package in workspace_meta.workspace_packages() {
                let details = CrateDetails::load(package)?;
                if let Some(other_details) = crates_map.get(&details.name) {
                    return Err(anyhow!(
                        "Crate parsed for {:?} has the same name of another crate parsed for {:?}",
                        details.manifest_path,
                        other_details.manifest_path,
                    ));
                }
                crates_map.insert(details.name.clone(), details);
            }
            crates_map
        };

        // All path dependencies should be members of the root workspace so that
        // they can be verified and checked in tandem.
        for details in crates_map.values() {
            for dep in details.deps_to_publish() {
                if !crates_map.contains_key(dep) {
                    return Err(anyhow!(
                        "Crate {} refers to path dependency {}, which could not be detected for the workspace of {:?}. You might need to add {} as a workspace member in {:?}.",
                        &details.name,
                        dep,
                        root.display(),
                        dep,
                        root.display()
                    ));
                }
            }
        }

        Ok(Crates { root, crates_map })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn publish(
        &self,
        krate: &String,
        crates_to_verify: &HashSet<&String>,
        after_publish_delay: Option<&u64>,
        last_publish_instant: &mut Option<Instant>,
        index_conf: Option<&CratesIoIndexConfiguration>,
        clear_cargo_home: Option<&String>,
        post_publish_cleanup_dirs: &[String],
    ) -> anyhow::Result<()> {
        let details = self
            .crates_map
            .get(krate)
            .with_context(|| format!("Crate not found: {krate}"))?;

        let should_verify = crates_to_verify.get(krate).is_some();

        info!("Preparing crate {krate} for publishing");
        details.prepare_for_publish()?;

        if let Some(last_publish_instant) = last_publish_instant {
            if let Some(after_publish_delay) = after_publish_delay {
                let after_publish_delay = Duration::from_secs(*after_publish_delay);
                let now = last_publish_instant.elapsed();
                if now < after_publish_delay {
                    let sleep_duration = after_publish_delay - now;
                    info!(
                        "Waiting for {:?} before publishing crate {krate} to avoid rate limits",
                        sleep_duration
                    );
                    thread::sleep(sleep_duration);
                }
            };
        }

        info!("Publishing crate {krate}");
        let mut spurious_network_err_count = 0;
        while let Err(err) = details.publish(should_verify) {
            match err {
                PublishError::RateLimited(err) => {
                    spurious_network_err_count = 0;
                    info!("`cargo publish` failed due to rate limiting: {err}");
                    // crates.io should give a new token every 1 minute, so
                    // sleep by that much and try again
                    let rate_limit_delay = Duration::from_secs(64);
                    info!(
                        "Sleeping for {:?} before trying to publish again",
                        rate_limit_delay
                    );
                    thread::sleep(rate_limit_delay);
                }
                PublishError::SpuriousNetworkError(err) => {
                    info!("`cargo publish` failed due to: {err}");
                    spurious_network_err_count += 1;
                    if spurious_network_err_count < 8 {
                        let rate_limit_delay = Duration::from_secs(30);
                        info!(
                            "Sleeping for {:?} before trying to publish again",
                            rate_limit_delay
                        );
                        thread::sleep(rate_limit_delay);
                    } else {
                        info!("cargo publish yielded too many network errors; it won't be retried");
                        return Err(anyhow!(err));
                    }
                }
                PublishError::Any(err) => {
                    return Err(err);
                }
            }
        }

        for cleanup_dir in post_publish_cleanup_dirs {
            if fs::metadata(cleanup_dir).is_ok() {
                fs::remove_dir_all(cleanup_dir)?;
            }
        }

        info!("Waiting for crate {} to be available on crates.io", krate);
        // Don't return until the crate has finished being published; it won't
        // be immediately visible on crates.io, so wait until it shows up.
        while !external::crates_io::does_crate_exist(krate, &details.version)? {
            thread::sleep(Duration::from_millis(1536))
        }

        if let Some(index_conf) = index_conf {
            info!(
                "Waiting for crate {} to be available in the registry",
                krate
            );
            while !external::crates_io::does_crate_exist_in_cratesio_index(
                index_conf,
                krate,
                &details.version,
            )? {
                thread::sleep(Duration::from_millis(1536))
            }
        }

        *last_publish_instant = Some(Instant::now());

        if let Some(cargo_home) = clear_cargo_home {
            fs::remove_dir_all(cargo_home)?;
            fs::create_dir_all(cargo_home)?;
        }

        if let Ok(crates_committed_file) = env::var("SPUB_CRATES_COMMITTED_FILE") {
            let mut did_warn_of_polling = false;
            loop {
                let crates_committed =
                    fs::read_to_string(&crates_committed_file).with_context(|| {
                        format!(
                            "Failed to read $SPUB_CRATES_COMMITTED_FILE ({})",
                            crates_committed_file
                        )
                    })?;
                for crate_committed in crates_committed.lines() {
                    if crate_committed == krate {
                        return Ok(());
                    }
                }
                if !did_warn_of_polling {
                    did_warn_of_polling = true;
                    info!("Polling $SPUB_CRATES_COMMITTED_FILE for crate {krate}");
                }
                thread::sleep(Duration::from_millis(128));
            }
        };

        Ok(())
    }

    pub fn what_needs_publishing<'a, Crate: AsRef<str>>(
        &self,
        krate: Crate,
        publish_order: &'a [String],
    ) -> anyhow::Result<Vec<&'a String>> {
        let mut registered_crates: HashSet<&str> = HashSet::new();
        fn register_crates<'b>(
            crates: &'b Crates,
            registered_crates: &mut HashSet<&'b str>,
            krate: &'b str,
        ) -> anyhow::Result<()> {
            if registered_crates.get(krate).is_none() {
                registered_crates.insert(krate);

                let details = crates
                    .crates_map
                    .get(krate)
                    .with_context(|| format!("Crate not found: {krate}"))?;

                for dep in details.deps_to_publish() {
                    register_crates(crates, registered_crates, dep)?;
                }
            }
            Ok(())
        }
        register_crates(self, &mut registered_crates, krate.as_ref())?;

        Ok(publish_order
            .iter()
            .filter(|krate| registered_crates.iter().any(|reg_crate| reg_crate == krate))
            .collect())
    }
}
