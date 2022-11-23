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

mod crate_details;
mod crates;
mod external;
mod git;
mod toml;
mod version;

use anyhow::Context;
use clap::{Parser, Subcommand};
use crates::Crates;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::{info, span, Level};
use tracing_subscriber::prelude::*;

use crate::git::{git_checkpoint, GCM};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(about = "Publish crates in order from least to most dependees")]
    Publish(PublishOpts),
}

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct PublishOpts {
    #[clap(long, help = "Path to the workspace root")]
    root: PathBuf,

    #[clap(
        short = 'c',
        long = "crate",
        help = "Select crates to be published. If empty, all crates in the workspace of --root will be published."
    )]
    crates: Vec<String>,

    #[clap(
        short = 's',
        long = "start-from",
        help = "Start publishing from this crate. Useful to resume the process in case it fails for some reason. This option does not take into account code changes between the stop of the first attempt and the resumption, so you might potentially miss some crates in case they're added and/or renamed within that gap."
    )]
    start_from: Option<String>,

    #[clap(
        short = 'e',
        long = "exclude",
        help = "Crates to be excluded from the publishing process."
    )]
    exclude: Vec<String>,

    #[clap(
        short = 'k',
        long = "post-check",
        help = "Run post checks, e.g. cargo check, after publishing."
    )]
    post_check: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_writer(std::io::stdout)
                .with_target(false),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_writer(std::io::stderr)
                .with_target(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::ERROR),
        )
        .init();

    let args = Args::parse();

    match args.command {
        Command::Publish(opts) => publish(opts),
    }
}

fn publish(opts: PublishOpts) -> anyhow::Result<()> {
    let mut crates = Crates::load_crates_in_workspace(opts.root.clone())?;
    crates.setup_crates()?;

    struct OrderedCrate {
        name: String,
        rank: usize,
    }
    let mut publish_order: Vec<OrderedCrate> = vec![];
    loop {
        let mut progressed = false;
        for (krate, details) in &crates.details {
            if publish_order
                .iter()
                .any(|ord_crate| ord_crate.name == *krate)
            {
                continue;
            }
            let deps: HashSet<&String> = HashSet::from_iter(details.deps_relevant_during_publish());
            let ordered_deps = publish_order
                .iter()
                .filter(|ord_crate| deps.iter().any(|dep| **dep == ord_crate.name))
                .collect::<Vec<_>>();
            if ordered_deps.len() == deps.len() {
                publish_order.push(OrderedCrate {
                    rank: ordered_deps.iter().fold(1usize, |acc, ord_crate| {
                        acc.checked_add(ord_crate.rank).unwrap()
                    }),
                    name: krate.into(),
                });
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    publish_order.sort_by(|a, b| {
        use std::cmp::Ordering;
        match a.rank.cmp(&b.rank) {
            Ordering::Equal => a.name.cmp(&b.name),
            other => other,
        }
    });
    let publish_order: Vec<String> = publish_order
        .into_iter()
        .map(|ord_crate| ord_crate.name)
        .collect();
    info!(
        "If we were to publish all crates, it would be in this order: {}",
        publish_order
            .iter()
            .map(|krate| krate.to_owned())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let unordered_crates = crates
        .details
        .keys()
        .filter(|krate| !publish_order.iter().any(|ord_crate| ord_crate == *krate))
        .collect::<Vec<_>>();
    if !unordered_crates.is_empty() {
        anyhow::bail!(
            "Failed to determine publish order for the following crates: {}",
            unordered_crates
                .iter()
                .map(|krate| (*krate).into())
                .collect::<Vec<String>>()
                .join(", ")
        );
    }

    let input_crates = if !opts.crates.is_empty() {
        opts.crates.clone()
    } else {
        publish_order
            .clone()
            .into_iter()
            .filter(|krate| {
                if opts
                    .exclude
                    .iter()
                    .any(|excluded_crate| excluded_crate == krate)
                {
                    return false;
                }

                let details = crates
                    .details
                    .get(krate)
                    .with_context(|| format!("Crate not found: {krate}"))
                    .unwrap();
                details.should_be_published
            })
            .collect()
    };
    let (selected_crates, selected_crates_order) = if let Some(start_from) = opts.start_from {
        let mut keep = false;
        let selected_crates = input_crates
            .into_iter()
            .filter(|krate| {
                if *krate == start_from {
                    keep = true;
                }
                keep
            })
            .collect::<Vec<_>>();

        let mut keep = false;
        let selected_crates_order = publish_order
            .iter()
            .filter(|krate| {
                if **krate == start_from {
                    keep = true;
                }
                keep && selected_crates.iter().any(|sel_crate| sel_crate == *krate)
            })
            .collect::<Vec<_>>();

        (selected_crates, selected_crates_order)
    } else {
        let selected_crates_order = publish_order
            .iter()
            .filter(|ord_crate| {
                input_crates
                    .iter()
                    .any(|sel_crate| *sel_crate == **ord_crate)
            })
            .collect::<Vec<_>>();

        (input_crates, selected_crates_order)
    };
    if selected_crates.is_empty() {
        anyhow::bail!("No crates could be selected from the CLI options");
    }

    info!(
        "Processing selected crates in this order: {}",
        selected_crates_order
            .iter()
            .map(|krate| (*krate).into())
            .collect::<Vec<String>>()
            .join(", ")
    );

    let unordered_selected_crates = selected_crates
        .iter()
        .filter(|sel_crate| {
            !selected_crates_order
                .iter()
                .any(|sel_crate_ordered| sel_crate_ordered == sel_crate)
        })
        .collect::<Vec<_>>();
    if !unordered_selected_crates.is_empty() {
        anyhow::bail!(
            "Failed to determine publish order for the following selected crates: {}",
            unordered_selected_crates
                .iter()
                .map(|krate| (*krate).into())
                .collect::<Vec<String>>()
                .join(", ")
        );
    }

    fn validate_crates(
        crates: &Crates,
        initial_crate: &String,
        parent_crate: Option<&String>,
        krate: &String,
        excluded_crates: &[String],
        visited_crates: &[&String],
    ) -> anyhow::Result<()> {
        if visited_crates
            .iter()
            .any(|visited_crate| *visited_crate == krate)
        {
            return Ok(());
        }

        if parent_crate.is_some() {
            if excluded_crates
                .iter()
                .any(|excluded_crate| excluded_crate == krate)
            {
                if let Some(parent_crate) = parent_crate {
                    anyhow::bail!("Crate {krate} was excluded from CLI options, but it is a dependency of {parent_crate}, and that is a dependency of {initial_crate}, which would be published.");
                } else {
                    anyhow::bail!("Crate {krate} was excluded from CLI options, but it is a dependency of {initial_crate}, which would be published.");
                }
            }
        } else if excluded_crates
            .iter()
            .any(|excluded_crate| excluded_crate == krate)
        {
            return Ok(());
        }

        let details = crates
            .details
            .get(krate)
            .with_context(|| format!("Crate not found: {krate}"))?;
        if !details.should_be_published {
            if let Some(parent_crate) = parent_crate {
                anyhow::bail!("Crate {krate} should not be published, but it is a dependency of {parent_crate}, and that is a dependency of {initial_crate}, which would be published. Check if {krate} has \"publish = false\" in {:?}.", details.toml_path);
            } else {
                anyhow::bail!("Crate {krate} should not be published, but it is a dependency of {initial_crate}, which would be published. Check if {krate} has \"publish = false\" in {:?}.", details.toml_path);
            }
        }

        for dep in &details.deps {
            let visited_crates = visited_crates
                .iter()
                .copied()
                .chain(vec![krate].into_iter())
                .collect::<Vec<_>>();
            validate_crates(
                crates,
                initial_crate,
                if krate == initial_crate {
                    None
                } else {
                    Some(krate)
                },
                dep,
                excluded_crates,
                &visited_crates,
            )?;
        }

        Ok(())
    }
    for krate in &selected_crates {
        info!("Validating crate {krate}");
        // validate_crates(&crates, krate, None, krate, &opts.exclude, &[])?;
    }

    if let Ok(registry) = std::env::var("SPUB_REGISTRY") {
        for (_, details) in crates.details.iter() {
            details.set_registry(&registry)?
        }
    }

    let mut processed_crates: HashSet<String> = HashSet::new();
    for sel_crate in selected_crates_order {
        let span = span!(Level::INFO, "_", crate = sel_crate);
        let _enter = span.enter();

        if processed_crates.get(sel_crate).is_some() {
            info!("Crate was already processed",);
            continue;
        }

        info!("Processing crate");

        {
            let details = crates.details.get(sel_crate).unwrap();
            for krate in &publish_order {
                if krate == sel_crate {
                    break;
                }
                let crate_details = crates
                    .details
                    .get(krate)
                    .with_context(|| format!("Crate details not found for crate: {krate}"))?;
                details.write_dependency_version(krate, &crate_details.version)?;
            }
        }

        let crates_to_publish = crates.what_needs_publishing(sel_crate, &publish_order)?;

        if crates_to_publish.is_empty() {
            info!("Crate does not need to be published");
            continue;
        } else if crates_to_publish.len() == 1 {
            info!("Publishing crate {}", crates_to_publish[0])
        } else {
            info!(
                "Crates will be processed in the following order for publishing {sel_crate}: {}",
                crates_to_publish
                    .iter()
                    .map(|krate| (krate).into())
                    .collect::<Vec<String>>()
                    .join(", ")
            );
        }

        for krate in crates_to_publish {
            if processed_crates.get(sel_crate).is_some() {
                info!("Crate {krate} was already processed",);
                continue;
            }

            let last_version = {
                let details = crates.details.get_mut(&krate).unwrap();
                let prev_versions = external::crates_io::crate_versions(&krate)?;
                if details.needs_publishing(&opts.root, &prev_versions)? {
                    git_checkpoint(&opts.root, GCM::Save)?;
                    details.maybe_bump_version(prev_versions)?;
                    let last_version = details.version.clone();
                    crates.strip_dev_deps_and_publish(&krate)?;
                    last_version
                } else {
                    info!("Crate {krate} does not need to be published");
                    details.version.clone()
                }
            };

            for (_, details) in crates.details.iter() {
                details.write_dependency_version(&krate, &last_version)?;
            }

            processed_crates.insert(krate);
        }

        processed_crates.insert(sel_crate.into());
    }

    if opts.post_check {
        let mut cmd = std::process::Command::new("cargo");
        let mut cmd = cmd.current_dir(&opts.root).arg("update");
        for krate in &processed_crates {
            cmd = cmd.arg("-p").arg(krate);
        }
        if !cmd.status()?.success() {
            anyhow::bail!("Command failed: {cmd:?}");
        };

        for (_, details) in crates.details.iter() {
            let mut cmd = std::process::Command::new("cargo");
            cmd.current_dir(&opts.root)
                .arg("check")
                .arg("-p")
                .arg(&details.name);
            if !cmd.status()?.success() {
                anyhow::bail!("Command failed: {cmd:?}");
            };
        }
    }

    Ok(())
}
