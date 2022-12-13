use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    time::Instant,
};

use anyhow::{anyhow, Context};
use clap::Parser;
use tracing::{info, span, Level};

use crate::{
    crate_details::CrateDetails,
    crates::{CrateName, Crates},
    external::{
        cargo::{cargo_check_crate, cargo_update_workspace},
        crates_io,
    },
    git::{git_hard_reset, git_head_sha, with_git_checkpoint, GitCheckpoint},
};

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
pub struct PublishOpts {
    #[clap(short = 'r', long, help = "Path to the workspace root")]
    root: PathBuf,

    #[clap(
        short = 'p',
        long = "publish-only",
        help = "Only publish this crate. If this option is not used, all crates in the target workspace will be considered for publishing. Can be specified multiple times."
    )]
    publish_only: Vec<String>,

    #[clap(
        short = 's',
        long = "start-from",
        help = "Start publishing from this crate. Useful to resume the process in case it fails for some reason. This option does not take into account crates which are ordered before the given crate, so you might potentially miiss added or renamed crates in case they are ordered before the given crate."
    )]
    start_from: Option<String>,

    #[clap(
        long = "verify-from",
        help = "When publishing, only verify crates starting from this crate. Useful to skip the verification process of all crates up to the given crate, which can be time-consuming if the crate depends on lots of other crates that are expensive to verify."
    )]
    verify_from: Option<String>,

    #[clap(
        short = 'v',
        long = "verify-only",
        help = "Only verify this crate before publishing. If this option is not used, all crates will be verified before publishing. Can be specified multiple times."
    )]
    verify_only: Vec<String>,

    #[clap(
        long = "after-publish-delay",
        help = "How many seconds to wait after publishing a crate. Useful to work around crates.io publishing rate limits in case you need to publish lots of crates."
    )]
    after_publish_delay: Option<u64>,

    #[clap(
        long = "include-crates-dependents",
        help = "Also include dependents of crates which were selected through the CLI"
    )]
    include_crates_dependents: bool,

    #[clap(
        short = 'e',
        long = "exclude",
        help = "Crates to be excluded from the publishing process"
    )]
    exclude: Vec<String>,

    #[clap(
        short = 'k',
        long = "post-check",
        help = "Run post checks, e.g. cargo check, after publishing"
    )]
    post_check: bool,

    #[clap(
        long = "for-pull-request",
        help = "Set up the changes such that the diff can be used for a pull request"
    )]
    for_pull_request: bool,
}

pub fn publish(opts: PublishOpts) -> anyhow::Result<()> {
    if env::var("CI").is_ok() {
        info!("Publishing has started");
    }

    let initial_commit = git_head_sha(&opts.root)?;

    let mut crates = Crates::load_workspace_crates(opts.root.clone())?;

    let publish_order = get_publish_order(&crates.crates_map);
    info!(
        "If we were to publish all crates, it would happen in this order: {}",
        publish_order
            .iter()
            .map(|krate| krate.to_owned())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let unordered_crates = crates
        .crates_map
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

    let crates_to_exclude = {
        let mut crates_to_exclude: HashSet<&String> = HashSet::from_iter(opts.exclude.iter());

        loop {
            let mut progressed = false;

            // Exclude also crates which depend on crates to be excluded
            let excluded_crates = crates_to_exclude
                .iter()
                .map(|excluded_crate| excluded_crate.to_owned())
                .collect::<Vec<_>>();
            for excluded_crate in excluded_crates {
                for krate in &publish_order {
                    let details = crates
                        .crates_map
                        .get(krate)
                        .with_context(|| format!("Crate not found: {krate}"))?;
                    if details.deps_to_publish().any(|dep| dep == excluded_crate) {
                        let inserted = crates_to_exclude.insert(krate);
                        if inserted {
                            info!(
                                "Excluding crate {} because it depends on {}",
                                krate, excluded_crate
                            );
                        }
                        progressed |= inserted;
                    }
                }
            }

            if !progressed {
                break;
            }
        }

        crates_to_exclude
    };

    let candidate_crates = if opts.publish_only.is_empty() {
        publish_order
            .iter()
            .filter_map(|krate| {
                if opts
                    .start_from
                    .as_ref()
                    .map(|start_from| start_from == krate)
                    .unwrap_or(false)
                {
                    return Some(Ok(krate));
                }
                if crates_to_exclude
                    .iter()
                    .any(|excluded_crate| *excluded_crate == krate)
                {
                    return None;
                }
                if let Some(details) = crates.crates_map.get(krate) {
                    if details.should_be_published {
                        Some(Ok(krate))
                    } else {
                        info!("Filtering out crate {krate} because it should not be published");
                        None
                    }
                } else {
                    Some(Err(anyhow!("Crate not found: {krate}")))
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        let mut crates_to_include: HashSet<&String> = HashSet::from_iter(opts.publish_only.iter());

        if opts.include_crates_dependents {
            loop {
                let mut progressed = false;

                let included_crates = crates_to_include
                    .iter()
                    .map(|krate| krate.to_owned())
                    .collect::<Vec<_>>();
                for included_crate in included_crates {
                    for krate in &publish_order {
                        if crates_to_exclude.get(krate).is_some() {
                            continue;
                        }
                        let details = crates
                            .crates_map
                            .get(krate)
                            .with_context(|| format!("Crate not found: {krate}"))?;
                        if details.should_be_published
                            && details.deps_to_publish().any(|dep| dep == included_crate)
                        {
                            let inserted = crates_to_include.insert(krate);
                            if inserted {
                                info!(
                                    "Including crate {} because it depends on {}",
                                    krate, included_crate
                                );
                            }
                            progressed |= inserted;
                        }
                    }
                }

                if !progressed {
                    break;
                }
            }
        }

        publish_order
            .iter()
            .filter(|ordered_crate| crates_to_include.get(ordered_crate).is_some())
            .collect::<Vec<_>>()
    };

    let selected_crates = if let Some(start_from) = opts.start_from {
        let mut candidate_crates = candidate_crates;
        let mut keep = false;
        candidate_crates.retain_mut(|krate| {
            if **krate == start_from {
                keep = true;
                if crates_to_exclude
                    .iter()
                    .any(|excluded_crate| excluded_crate == krate)
                {
                    return false;
                }
            }
            keep
        });
        candidate_crates
    } else {
        candidate_crates
    };
    if selected_crates.is_empty() {
        anyhow::bail!("No crates could be selected from the CLI options");
    }

    info!(
        "Selected the following crates to be published, in order: {}",
        selected_crates
            .iter()
            .map(|krate| (*krate).into())
            .collect::<Vec<String>>()
            .join(", ")
    );

    fn validate_crates(
        crates: &Crates,
        initial_crate: &String,
        parent_crate: Option<&String>,
        krate: &String,
        excluded_crates: &HashSet<&String>,
        visited_crates: &[&String],
    ) -> anyhow::Result<()> {
        if visited_crates
            .iter()
            .any(|visited_crate| *visited_crate == krate)
        {
            return Ok(());
        }

        if excluded_crates
            .iter()
            .any(|excluded_crate| *excluded_crate == krate)
        {
            if let Some(parent_crate) = parent_crate {
                anyhow::bail!("Crate {krate} was excluded from CLI options, but it is a dependency of {parent_crate}, and that is a dependency of {initial_crate}, which would be published.");
            } else {
                anyhow::bail!("Crate {krate} was excluded from CLI options, but it is a dependency of  {initial_crate}, which would be published.");
            }
        }

        let details = crates
            .crates_map
            .get(krate)
            .with_context(|| format!("Crate not found: {krate}"))?;
        if !details.should_be_published {
            if let Some(parent_crate) = parent_crate {
                anyhow::bail!("Crate {krate} should not be published, but it is a dependency of {parent_crate}, and that is a dependency of {initial_crate}, which would be published. Check if {krate} has \"publish = false\" in {:?}.", details.manifest_path);
            } else {
                anyhow::bail!("Crate {krate} should not be published, but it is a dependency of {initial_crate}, which would be published. Check if {krate} has \"publish = false\" in {:?}.", details.manifest_path);
            }
        }

        for dep in details.deps_to_publish() {
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
        validate_crates(&crates, krate, None, krate, &crates_to_exclude, &[])?;
    }

    if let Ok(registry) = env::var("SPUB_REGISTRY") {
        for (_, details) in crates.crates_map.iter() {
            details.set_registry(&registry)?
        }
    }

    let crates_to_verify = {
        let mut crates_to_verify = if opts.verify_only.is_empty() {
            HashSet::from_iter(publish_order.iter())
        } else {
            HashSet::from_iter(opts.verify_only.iter())
        };
        if let Some(verify_from) = opts.verify_from {
            let mut keep = false;
            for krate in &publish_order {
                if *krate == verify_from {
                    keep = true;
                }
                if keep {
                    crates_to_verify.insert(krate);
                }
            }
        }
        crates_to_verify
    };

    let mut processed_crates: HashSet<&String> = HashSet::new();
    let mut last_publish_instant: Option<Instant> = None;
    for sel_crate in selected_crates {
        let span = span!(Level::INFO, "_", crate = sel_crate);
        let _enter = span.enter();

        if processed_crates.get(sel_crate).is_some() {
            info!("Crate was already processed",);
            continue;
        }

        info!("Processing crate");

        with_git_checkpoint(&opts.root, GitCheckpoint::Save, || -> anyhow::Result<()> {
            let details = crates
                .crates_map
                .get(sel_crate)
                .with_context(|| format!("Crate not found: {sel_crate}"))?;
            for prev_crate in &publish_order {
                if prev_crate == sel_crate {
                    break;
                }
                let prev_crate_details = crates
                    .crates_map
                    .get(prev_crate)
                    .with_context(|| format!("Crate not found: {prev_crate}"))?;
                details.write_dependency_version(prev_crate, &prev_crate_details.version, false)?;
            }
            Ok(())
        })??;

        let crates_to_publish = crates.what_needs_publishing(sel_crate, &publish_order)?;

        if crates_to_publish.is_empty() {
            info!("Crate does not need to be published");
            continue;
        } else if crates_to_publish.len() == 1 {
            info!("Preparing to publish {}", crates_to_publish[0])
        } else {
            info!(
                "Crates will be taken into account in the following order for publishing {sel_crate}: {}",
                crates_to_publish
                    .iter()
                    .map(|krate| (*krate).into())
                    .collect::<Vec<String>>()
                    .join(", ")
            );
        }

        let (already_processed_crates, crates_to_publish): (Vec<&String>, Vec<&String>) =
            crates_to_publish
                .into_iter()
                .partition(|krate| processed_crates.get(*krate).is_some());

        if !already_processed_crates.is_empty() {
            info!(
                "The following crates have already been processed: {}",
                already_processed_crates
                    .iter()
                    .map(|krate| (*krate).into())
                    .collect::<Vec<String>>()
                    .join(", ")
            );
        }

        for krate in crates_to_publish {
            let crate_version = {
                let prev_versions = crates_io::crate_versions(krate)?;

                let details = crates
                    .crates_map
                    .get_mut(krate)
                    .with_context(|| format!("Crate not found: {krate}"))?;

                details.adjust_version(&prev_versions)?;

                if details.needs_publishing(&opts.root)? {
                    with_git_checkpoint(&opts.root, GitCheckpoint::Save, || {
                        details.maybe_bump_version(
                            prev_versions
                                .into_iter()
                                .map(|prev_version| prev_version.version)
                                .collect(),
                        )
                    })??;
                    let version = details.version.clone();
                    crates.publish(
                        krate,
                        &crates_to_verify,
                        opts.after_publish_delay.as_ref(),
                        &mut last_publish_instant,
                    )?;
                    version
                } else {
                    info!("Crate {krate} does not need to be published");
                    details.version.clone()
                }
            };

            with_git_checkpoint(&opts.root, GitCheckpoint::Save, || -> anyhow::Result<()> {
                for (_, details) in crates.crates_map.iter() {
                    details.write_dependency_version(krate, &crate_version, true)?;
                }
                Ok(())
            })??;

            processed_crates.insert(krate);
        }

        processed_crates.insert(sel_crate);
    }

    if opts.post_check {
        let ordered_processed_crates = publish_order
            .iter()
            .filter(|krate| processed_crates.get(krate).is_some())
            .collect::<Vec<_>>();
        info!(
            "Processed the following crates (ordered by publishing order): {}",
            ordered_processed_crates
                .iter()
                .map(|krate| (*krate).into())
                .collect::<Vec<String>>()
                .join(", ")
        );
        for krate in ordered_processed_crates {
            info!("Checking crate {krate}");
            let details = crates
                .crates_map
                .get(krate)
                .with_context(|| format!("Crate not found: {krate}"))?;
            cargo_check_crate(&details.manifest_path)?;
        }
    }

    if opts.for_pull_request {
        git_hard_reset(&opts.root, &initial_commit)?;

        for (_, details) in crates.crates_map.iter() {
            if details.should_be_published {
                for (_, other_details) in crates.crates_map.iter() {
                    if other_details.should_be_published {
                        other_details.write_dependency_version(
                            &details.name,
                            &details.version,
                            false,
                        )?;
                    }
                }
            }
        }

        cargo_update_workspace(&opts.root)?;
    }

    Ok(())
}

/// Produces the crates' publishing order from least to most dependents,
/// tiebreaking by natural sorting order based on the crates' names
fn get_publish_order(details: &HashMap<CrateName, CrateDetails>) -> Vec<String> {
    let mut publish_order: Vec<OrderedCrate> = vec![];

    struct OrderedCrate {
        name: String,
        rank: usize,
    }
    loop {
        let mut progressed = false;
        for (krate, details) in details {
            if publish_order
                .iter()
                .any(|ord_crate| ord_crate.name == *krate)
            {
                continue;
            }
            let deps: HashSet<&String> = HashSet::from_iter(details.deps_to_publish());
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

    publish_order
        .into_iter()
        .map(|ord_crate| ord_crate.name)
        .collect()
}

#[test]
#[cfg(feature = "test-0")]
fn test_get_publish_order() {
    use std::collections::HashMap;

    use crate::crate_details::CrateDetails;

    /*
       Case: BA depends on A, thus the order becomes A -> BA
    */
    let crate_a_name = "A";
    let crate_a = CrateDetails::new_for_testing(crate_a_name.into());

    let crate_ba_name = "BA";
    let mut crate_ba = CrateDetails::new_for_testing(crate_ba_name.into());
    crate_ba.deps.insert(crate_a_name.to_owned());

    assert_eq!(
        get_publish_order(&HashMap::from_iter(
            [
                (crate_a_name.into(), crate_a.clone()),
                (crate_ba_name.into(), crate_ba.clone()),
            ]
            .into_iter(),
        )),
        vec![crate_a_name.to_owned(), crate_ba_name.to_owned()]
    );

    /*
       Case: BB and BA both depend on A; tiebreak between BB and BA by name,
       thus the order becomes A -> BA -> BB
    */
    let crate_bb_name = "BB";
    let mut crate_bb = CrateDetails::new_for_testing(crate_bb_name.into());
    crate_bb.deps.insert(crate_a_name.to_owned());

    assert_eq!(
        get_publish_order(&HashMap::from_iter(
            [
                (crate_a_name.into(), crate_a.clone()),
                (crate_ba_name.into(), crate_ba.clone()),
                (crate_bb_name.into(), crate_bb.clone()),
            ]
            .into_iter(),
        )),
        vec![
            crate_a_name.to_owned(),
            crate_ba_name.to_owned(),
            crate_bb_name.to_owned()
        ]
    );

    /*
       Case: C depends on BA, thus the order becomes A -> BA -> BB -> C
    */
    let crate_c_name = "C";
    let mut crate_c = CrateDetails::new_for_testing(crate_c_name.into());
    crate_c.deps.insert(crate_ba_name.to_owned());

    assert_eq!(
        get_publish_order(&HashMap::from_iter(
            [
                (crate_a_name.into(), crate_a),
                (crate_ba_name.into(), crate_ba.clone()),
                (crate_bb_name.into(), crate_bb.clone()),
                (crate_c_name.into(), crate_c.clone()),
            ]
            .into_iter(),
        )),
        vec![
            crate_a_name.to_owned(),
            crate_ba_name.to_owned(),
            crate_bb_name.to_owned(),
            crate_c_name.to_owned()
        ]
    );
}
