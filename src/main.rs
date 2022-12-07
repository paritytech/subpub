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
mod publish;
mod toml;
mod version;

use std::{env, io};

use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

use publish::*;

fn main() -> anyhow::Result<()> {
    setup_tracing();

    let args = Args::parse();

    match args.command {
        Command::Publish(opts) => publish(opts),
    }
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(about = "Publish crates in order from least to most dependents")]
    Publish(PublishOpts),
}

fn setup_tracing() {
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
            .from_env_lossy(),
    );
    if env::var("CI").is_ok() {
        subscriber
            .with(
                tracing_subscriber::fmt::layer()
                    .with_file(true)
                    .with_line_number(true)
                    .with_writer(io::stdout)
                    .with_target(false),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_file(true)
                    .with_line_number(true)
                    .with_writer(io::stderr)
                    .with_target(false)
                    .with_filter(tracing_subscriber::filter::LevelFilter::ERROR),
            )
            .init();
    } else {
        subscriber
            .with(
                tracing_subscriber::fmt::layer()
                    .without_time()
                    .with_writer(io::stdout)
                    .with_target(false),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .without_time()
                    .with_writer(io::stderr)
                    .with_target(false)
                    .with_filter(tracing_subscriber::filter::LevelFilter::ERROR),
            )
            .init();
    };
}
