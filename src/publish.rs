use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
    time::Instant,
};

use anyhow::{anyhow, Context};
use clap::Parser;
use semver::Version;
use strum::EnumString;
use tracing::{info, span, Level};

use crate::{
    cargo::cargo_update_workspace,
    crate_details::CrateDetails,
    crates::{CrateName, CratesWorkspace},
    crates_io::{self, CratesIoCrateVersion, CratesIoIndexConfiguration},
    git::{git_hard_reset, git_head_sha},
    version::VersionBumpHeuristic,
};

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
pub struct PublishOpts {
    #[clap(short = 'r', long, help = "Path to the workspace root")]
    root: PathBuf,

    #[clap(
        short = 'p',
        long = "publish-only",
        help = "Only publish this crate. If this option is not used, all crates in the target workspace will be considered for publishing. Note that dependencies of the selected crates will also be considered for publishing. Can be specified multiple times."
    )]
    publish_only: Vec<String>,

    #[clap(
        short = 's',
        long = "start-from",
        help = "Start publishing from this crate. Useful to resume the process in case it fails for some reason. If the source code you're publishing has changed, there's a risk of missing new and/or modified crates if they're ordered before the one provided by this option."
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

    #[clap(
        long = "index-url",
        help = "The index API to check after publishing crates"
    )]
    index_url: Option<String>,

    #[clap(
        long = "index-repository",
        help = "The index API to check after publishing crates"
    )]
    index_repository: Option<String>,

    #[clap(
        long = "clear-cargo-home",
        help = "The $CARGO_HOME directory to clear after publishing a crate"
    )]
    clear_cargo_home: Option<String>,

    #[clap(long = "stop-at-step", help = "The step to stop at")]
    stop_at_step: Option<String>,

    #[clap(
        long = "bump-compatible",
        help = "Provide the name for a crate which should be bumped to a compatible version IF it needs to be bumped before publishing. Dependents of those crates will be also bumped to a compatible version ONLY IF all of their dependencies have been bumped compatibly. Can be specified multiple times."
    )]
    crates_to_bump_compatibly: Vec<String>,

    #[clap(
        long = "bump-major",
        help = "Provide the name for a crate which should be bumped to a major version IF it needs to be bumped before publishing. This option takes precedence over --bump-compatible. Can be specified multiple times."
    )]
    crates_to_bump_majorly: Vec<String>,

    #[clap(
        long = "pre-bump-version",
        help = "Given in the form [crate]=[version]. Sets the crate to the given version before processing it. Can be specified multiple times."
    )]
    pre_bump_versions: Vec<String>,

    #[clap(
        long = "publish-version",
        help = "Given in the form [crate]=[version]. Sets the crate to a fixed version which will not change before publishing it. This option takes precedence over --pre-bump-version. Can be specified multiple times."
    )]
    publish_versions: Vec<String>,

    #[clap(
        long = "disable-version-adjustment",
        help = "Disable any version adjustments, i.e. versions will be published exactly as they are in the source code."
    )]
    no_version_adjustment: bool,

    #[clap(
        long = "verify-none",
        help = "Disable crate verification before publishing. Takes precedence over --verify-only."
    )]
    verify_none: bool,

    #[clap(
        long = "crate-debug-description",
        help = "Given in the form [crate]=[description]. Attach the given description to the crate to be used for debugging purposes."
    )]
    crates_debug_descriptions: Vec<String>,

    #[clap(
        long = "set-dependency-version",
        help = "Given in the form [crate]=[version]. Sets a crate dependency to a given version. Can be specified multiple times."
    )]
    set_dependency_versions: Vec<String>,

    #[clap(
        long = "post-publish-cleanup-glob",
        help = "Defines the glob patterns for files or directories which should be cleaned up after publishing crates. Can be specified multiple times."
    )]
    post_publish_cleanup_glob: Vec<String>,
}

#[derive(EnumString, strum::Display, PartialEq, Eq)]
enum StepToStopAt {
    #[strum(to_string = "validation")]
    Validation,
}

pub fn publish(opts: PublishOpts) -> anyhow::Result<()> {
    let index_conf = match (opts.index_url.as_ref(), opts.index_repository.as_ref()) {
        (Some(url), Some(repository)) => Some(CratesIoIndexConfiguration { url, repository }),
        (Some(_), _) => return Err(anyhow!("Specify --index-repository if using --index-url")),
        (_, Some(_)) => return Err(anyhow!("Specify --index-url if using --index-repository")),
        _ => None,
    };

    let pre_bump_versions = {
        let mut pre_bump_versions: HashMap<String, Version> = HashMap::new();

        for arg in opts.pre_bump_versions {
            let (krate, raw_version) = {
                let mut parts = arg.split('=');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(krate), Some(raw_version), None) => (krate, raw_version),
                    _ => return Err(anyhow!(
                            "Argument \"{}\" of --pre-bump-version should be given in the form [crate]=[version]",
                            arg
                            )
                        )
                }
            };
            let version = Version::parse(raw_version).with_context(|| {
                format!(
                    "Version \"{}\" from argument \"{}\" of --pre-bump-version could not be parsed as SemVer",
                    arg,
                    raw_version
                )
            })?;
            pre_bump_versions.insert(krate.into(), version);
        }

        pre_bump_versions
    };

    let crates_debug_descriptions = {
        let mut crates_debug_descriptions: HashMap<String, String> = HashMap::new();

        for arg in opts.crates_debug_descriptions {
            let (krate, description) = {
                let mut parts = arg.split('=');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(krate), Some(raw_version), None) => (krate, raw_version),
                    _ => return Err(anyhow!(
                            "Argument \"{}\" of --pre-bump-version should be given in the form [crate]=[version]",
                            arg
                            )
                        )
                }
            };
            crates_debug_descriptions.insert(krate.into(), description.into());
        }

        crates_debug_descriptions
    };

    let publish_versions = {
        let mut publish_versions: HashMap<String, Version> = HashMap::new();

        for arg in opts.publish_versions {
            let (krate, raw_version) = {
                let mut parts = arg.split('=');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(krate), Some(raw_version), None) => (krate, raw_version),
                    _ => return Err(anyhow!(
                            "Argument \"{}\" of --pre-bump-version should be given in the form [crate]=[version]",
                            arg
                        ))
                }
            };
            let version = Version::parse(raw_version).with_context(|| {
                format!(
                    "Version \"{}\" from argument \"{}\" of --pre-bump-version could not be parsed as SemVer",
                    arg,
                    raw_version
                )
            })?;
            publish_versions.insert(krate.into(), version);
        }

        publish_versions
    };

    let set_dependency_versions = {
        let mut set_dependency_versions: HashMap<String, Version> = HashMap::new();

        for arg in opts.set_dependency_versions {
            let (krate, raw_version) = {
                let mut parts = arg.split('=');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(krate), Some(raw_version), None) => (krate, raw_version),
                    _ => return Err(anyhow!(
                            "Argument \"{}\" of --pre-bump-version should be given in the form [crate]=[version]",
                            arg
                        ))
                }
            };
            let version = Version::parse(raw_version).with_context(|| {
                format!(
                    "Version \"{}\" from argument \"{}\" of --pre-bump-version could not be parsed as SemVer",
                    arg,
                    raw_version
                )
            })?;
            set_dependency_versions.insert(krate.into(), version);
        }

        set_dependency_versions
    };

    let stop_at_step = if let Some(step) = opts.stop_at_step.as_ref() {
        Some(
            StepToStopAt::from_str(step)
                .with_context(|| format!("Invalid step for --stop-at-step: {}", step))?,
        )
    } else {
        None
    };

    info!("Publishing has started");

    let initial_commit = git_head_sha(&opts.root)?;

    let mut workspace = CratesWorkspace::load(opts.root.clone())?;

    let publish_order = get_publish_order(&workspace.crates);
    info!(
        "If we were to publish all crates, it would happen in this order: {}",
        publish_order
            .iter()
            .map(|krate| krate.to_owned())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let unordered_crates = workspace
        .crates
        .keys()
        .filter(|krate| !publish_order.iter().any(|ord_crate| ord_crate == *krate))
        .collect::<Vec<_>>();
    if !unordered_crates.is_empty() {
        return Err(anyhow!(
            "Failed to determine publish order for the following crates: {}",
            unordered_crates
                .iter()
                .map(|krate| (*krate).into())
                .collect::<Vec<String>>()
                .join(", ")
        ));
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
                    let details = workspace
                        .crates
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
                if let Some(details) = workspace.crates.get(krate) {
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
                        let details = workspace
                            .crates
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
        return Err(anyhow!("No crates could be selected from the CLI options"));
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
        workspace: &CratesWorkspace,
        crates_debug_descriptions: &HashMap<String, String>,
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

        fn get_crate_debug_description(
            debug_descriptions: &HashMap<String, String>,
            krate: &String,
        ) -> String {
            if let Some(debug_annotation) = debug_descriptions.get(krate) {
                format!(" NOTE!: {}.", debug_annotation)
            } else {
                "".into()
            }
        }

        if excluded_crates
            .iter()
            .any(|excluded_crate| *excluded_crate == krate)
        {
            if let Some(parent_crate) = parent_crate {
                return Err(anyhow!(
                    "Crate {} was excluded from CLI options, but it is a dependency of {}, and that is a dependency of {}, which would be published.{}",
                    krate,
                    parent_crate,
                    initial_crate,
                    get_crate_debug_description(crates_debug_descriptions, krate)
                ));
            } else {
                return Err(anyhow!(
                    "Crate {} was excluded from CLI options, but it is a dependency of {}, which would be published.{}",
                    krate,
                    initial_crate,
                    get_crate_debug_description(crates_debug_descriptions, krate)
                ));
            }
        }

        let details = workspace
            .crates
            .get(krate)
            .with_context(|| format!("Crate not found: {krate}"))?;
        if !details.should_be_published {
            if krate == initial_crate {
                return Err(anyhow!(
                    "Crate {} should not be published. Check if it has \"publish = false\" in {:?}.{}",
                    krate,
                    details.manifest_path,
                    get_crate_debug_description(crates_debug_descriptions, krate)
                ));
            } else if let Some(parent_crate) = parent_crate {
                return Err(anyhow!(
                    "Crate {} should not be published, but it is a dependency of {}, and that is a dependency of {}, which would be published. Check if {} has \"publish = false\" in {:?}.{}",
                    krate,
                    parent_crate,
                    initial_crate,
                    krate,
                    details.manifest_path,
                    get_crate_debug_description(crates_debug_descriptions, krate)
                ));
            } else {
                return Err(anyhow!(
                    "Crate {} should not be published, but it is a dependency of {}, which would be published. Check if {} has \"publish = false\" in {:?}.{}",
                    krate,
                    initial_crate,
                    krate,
                    details.manifest_path,
                    get_crate_debug_description(crates_debug_descriptions, krate)
                ));
            }
        }

        for dep in details.deps_to_publish() {
            let visited_crates = visited_crates
                .iter()
                .copied()
                .chain(vec![krate].into_iter())
                .collect::<Vec<_>>();
            validate_crates(
                workspace,
                crates_debug_descriptions,
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

    let crates_validation_errors = {
        let mut crates_validation_errors: HashMap<&String, String> = HashMap::new();
        for krate in &selected_crates {
            info!("Validating crate {krate}");
            if let Err(err) = validate_crates(
                &workspace,
                &crates_debug_descriptions,
                krate,
                None,
                krate,
                &crates_to_exclude,
                &[],
            ) {
                crates_validation_errors.insert(krate, err.to_string());
            }
        }
        crates_validation_errors
    };
    if !crates_validation_errors.is_empty() {
        for (krate, error) in crates_validation_errors {
            info!(
                "Validation of crate {} failed due to error: {}",
                krate, error
            );
        }
        return Err(anyhow!("Crates validation failed"));
    }

    if stop_at_step == Some(StepToStopAt::Validation) {
        return Ok(());
    }

    for (dep, version) in set_dependency_versions {
        for (_, details) in workspace.crates.iter() {
            details.write_dependency_version(
                &opts.root,
                &dep,
                &version,
                &["git", "branch", "rev", "tag", "path"],
            )?;
        }
    }

    let crates_to_verify = {
        let mut crates_to_verify = if opts.verify_none {
            HashSet::new()
        } else if opts.verify_only.is_empty() {
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

        if opts.include_crates_dependents {
            loop {
                let mut progressed = false;

                let included_crates = crates_to_verify
                    .iter()
                    .map(|krate| krate.to_owned())
                    .collect::<Vec<_>>();
                for included_crate in included_crates {
                    for krate in &publish_order {
                        let details = workspace
                            .crates
                            .get(krate)
                            .with_context(|| format!("Crate not found: {krate}"))?;
                        if details.should_be_published
                            && details.deps_to_publish().any(|dep| dep == included_crate)
                        {
                            let inserted = crates_to_verify.insert(krate);
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

        crates_to_verify
    };

    let mut crate_bump_heuristic: HashMap<&String, VersionBumpHeuristic> = HashMap::new();
    let mut processed_crates: HashSet<&String> = HashSet::new();
    let mut last_publish_instant: Option<Instant> = None;
    for sel_crate in selected_crates {
        let span = span!(Level::INFO, "_", crate = sel_crate);
        let _enter = span.enter();

        if processed_crates.get(sel_crate).is_some() {
            info!("Crate was already processed",);
            continue;
        }

        let should_adjust_version = {
            if opts.no_version_adjustment {
                false
            } else if let Some(set_version) = publish_versions.get(sel_crate) {
                let details = workspace
                    .crates
                    .get_mut(sel_crate)
                    .with_context(|| format!("Crate not found: {sel_crate}"))?;
                details.write_own_version(set_version.to_owned())?;
                false
            } else if let Some(pre_bump_version) = pre_bump_versions.get(sel_crate) {
                let details = workspace
                    .crates
                    .get_mut(sel_crate)
                    .with_context(|| format!("Crate not found: {sel_crate}"))?;
                details.write_own_version(pre_bump_version.to_owned())?;
                true
            } else {
                true
            }
        };

        info!("Processing crate");

        let details = workspace
            .crates
            .get(sel_crate)
            .with_context(|| format!("Crate not found: {sel_crate}"))?;
        for prev_crate in &publish_order {
            if prev_crate == sel_crate {
                break;
            }
            let prev_crate_details = workspace
                .crates
                .get(prev_crate)
                .with_context(|| format!("Crate not found: {prev_crate}"))?;
            details.write_dependency_version(
                &opts.root,
                prev_crate,
                &prev_crate_details.version,
                &[],
            )?;
        }

        let crates_to_publish = workspace.what_needs_publishing(sel_crate, &publish_order)?;

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
            enum VersionAdjustment {
                BasedOnPreviousVersions(Vec<CratesIoCrateVersion>),
                No,
            }
            let version_adjustment = if should_adjust_version {
                let prev_versions = crates_io::crate_versions(krate)?;
                VersionAdjustment::BasedOnPreviousVersions(prev_versions)
            } else {
                VersionAdjustment::No
            };

            let did_adjust_version = {
                match version_adjustment {
                    VersionAdjustment::BasedOnPreviousVersions(ref prev_versions) => {
                        let details = workspace
                            .crates
                            .get_mut(krate)
                            .with_context(|| format!("Crate not found: {krate}"))?;
                        details.adjust_version(prev_versions)?
                    }
                    VersionAdjustment::No => false,
                }
            };

            if did_adjust_version {
                let details = workspace
                    .crates
                    .get(krate)
                    .with_context(|| format!("Crate not found: {krate}"))?;
                for (_, other_details) in workspace.crates.iter() {
                    other_details.write_dependency_version(
                        &opts.root,
                        krate,
                        &details.version,
                        &[],
                    )?;
                }
            }

            let crate_version = {
                let details = workspace
                    .crates
                    .get_mut(krate)
                    .with_context(|| format!("Crate not found: {krate}"))?;
                if details.needs_publishing(None)? {
                    match version_adjustment {
                        VersionAdjustment::BasedOnPreviousVersions(prev_versions) => {
                            let bump_heuristic = if opts
                                .crates_to_bump_majorly
                                .iter()
                                .any(|some_crate| some_crate == krate)
                            {
                                VersionBumpHeuristic::Breaking
                            } else if opts
                                .crates_to_bump_compatibly
                                .iter()
                                .any(|some_crate| some_crate == krate)
                            {
                                VersionBumpHeuristic::Compatible
                            } else if let Some(dep_bumped_compatibly) =
                                details.deps_to_publish().find(|dep| {
                                    crate_bump_heuristic.get(dep)
                                        == Some(&VersionBumpHeuristic::Compatible)
                                })
                            {
                                // Dependencies only default to being bumped
                                // compatibly if their dependencies were also bumped
                                // compatibly
                                if let Some(dep_bumped_with_breaking_change) =
                                    details.deps_to_publish().find(|dep| {
                                        crate_bump_heuristic.get(dep)
                                            == Some(&VersionBumpHeuristic::Breaking)
                                    })
                                {
                                    info!(
                                        "`{}` and `{}` are dependencies of `{}`; `{}` was bumped with a compatible change, but `{}` was bumped with a breaking change, therefore `{}` will be bumped with a breaking change as well",
                                        dep_bumped_compatibly,
                                        dep_bumped_with_breaking_change,
                                        krate,
                                        dep_bumped_compatibly,
                                        dep_bumped_with_breaking_change,
                                        krate
                                    );
                                    VersionBumpHeuristic::Breaking
                                } else {
                                    VersionBumpHeuristic::Compatible
                                }
                            } else {
                                VersionBumpHeuristic::Breaking
                            };

                            details.maybe_bump_version(
                                prev_versions.into_iter().map(|vers| vers.version).collect(),
                                &bump_heuristic,
                            )?;
                            crate_bump_heuristic.insert(krate, bump_heuristic);
                        }
                        VersionAdjustment::No => (),
                    }

                    let version = details.version.clone();
                    workspace.publish(
                        krate,
                        &crates_to_verify,
                        opts.after_publish_delay.as_ref(),
                        &mut last_publish_instant,
                        index_conf.as_ref(),
                        opts.clear_cargo_home.as_ref(),
                        &opts.post_publish_cleanup_glob,
                    )?;
                    version
                } else {
                    info!("Crate {krate} does not need to be published");
                    details.version.clone()
                }
            };

            for (_, details) in workspace.crates.iter() {
                details.write_dependency_version(&opts.root, krate, &crate_version, &["path"])?;
            }

            processed_crates.insert(krate);
        }

        processed_crates.insert(sel_crate);
    }

    if opts.for_pull_request {
        info!("Preparing diff for a pull request");

        git_hard_reset(&opts.root, &initial_commit)?;

        for krate in processed_crates {
            {
                let details = workspace
                    .crates
                    .get_mut(krate)
                    .with_context(|| format!("Crate not found: {krate}"))?;
                details.write_own_version(details.version.clone())?;
            }
            let details = workspace
                .crates
                .get(krate)
                .with_context(|| format!("Crate not found: {krate}"))?;
            for (_, other_details) in workspace.crates.iter() {
                other_details.write_dependency_version(&opts.root, krate, &details.version, &[])?;
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
