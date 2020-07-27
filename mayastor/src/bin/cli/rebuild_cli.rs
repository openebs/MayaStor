//!
//! methods to interact with the rebuild process

use crate::context::Context;
use ::rpc::mayastor as rpc;
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};
use tonic::Status;

pub async fn handler(
    ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    match matches.subcommand() {
        ("start", Some(args)) => start(ctx, &args).await,
        ("stop", Some(args)) => stop(ctx, &args).await,
        ("pause", Some(args)) => pause(ctx, &args).await,
        ("resume", Some(args)) => resume(ctx, &args).await,
        ("state", Some(args)) => state(ctx, &args).await,
        ("progress", Some(args)) => progress(ctx, &args).await,
        (cmd, _) => {
            Err(Status::not_found(format!("command {} does not exist", cmd)))
        }
    }
}

pub fn subcommands<'a, 'b>() -> App<'a, 'b> {
    let start = SubCommand::with_name("start")
        .about("starts a rebuild")
        .arg(
            Arg::with_name("uuid")
                .required(true)
                .index(1)
                .help("uuid of the nexus"),
        )
        .arg(
            Arg::with_name("uri")
                .required(true)
                .index(2)
                .help("uri of child to start rebuilding"),
        );

    let stop = SubCommand::with_name("stop")
        .about("stops a rebuild")
        .arg(
            Arg::with_name("uuid")
                .required(true)
                .index(1)
                .help("uuid of the nexus"),
        )
        .arg(
            Arg::with_name("uri")
                .required(true)
                .index(2)
                .help("uri of child to stop rebuilding"),
        );

    let pause = SubCommand::with_name("pause")
        .about("pauses a rebuild")
        .arg(
            Arg::with_name("uuid")
                .required(true)
                .index(1)
                .help("uuid of the nexus"),
        )
        .arg(
            Arg::with_name("uri")
                .required(true)
                .index(2)
                .help("uri of child to pause rebuilding"),
        );

    let resume = SubCommand::with_name("resume")
        .about("resumes a rebuild")
        .arg(
            Arg::with_name("uuid")
                .required(true)
                .index(1)
                .help("uuid of the nexus"),
        )
        .arg(
            Arg::with_name("uri")
                .required(true)
                .index(2)
                .help("uri of child to resume rebuilding"),
        );

    let state = SubCommand::with_name("state")
        .about("gets the rebuild state of the child")
        .arg(
            Arg::with_name("uuid")
                .required(true)
                .index(1)
                .help("uuid of the nexus"),
        )
        .arg(
            Arg::with_name("uri")
                .required(true)
                .index(2)
                .help("uri of child to get the rebuild state from"),
        );

    let progress = SubCommand::with_name("progress")
        .about("shows the progress of a rebuild")
        .arg(
            Arg::with_name("uuid")
                .required(true)
                .index(1)
                .help("uuid of the nexus"),
        )
        .arg(
            Arg::with_name("uri")
                .required(true)
                .index(2)
                .help("uri of child to get the rebuild progress from"),
        );

    SubCommand::with_name("rebuild")
        .settings(&[
            AppSettings::SubcommandRequiredElseHelp,
            AppSettings::ColoredHelp,
            AppSettings::ColorAlways,
        ])
        .about("Rebuild management")
        .subcommand(start)
        .subcommand(stop)
        .subcommand(pause)
        .subcommand(resume)
        .subcommand(state)
        .subcommand(progress)
}

async fn start(
    mut ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    let uuid = matches.value_of("uuid").unwrap().to_string();
    let uri = matches.value_of("uri").unwrap().to_string();

    ctx.v2(&format!(
        "Starting rebuild of child {} on nexus {}",
        uri, uuid
    ));
    ctx.client
        .start_rebuild(rpc::StartRebuildRequest {
            uuid: uuid.clone(),
            uri: uri.clone(),
        })
        .await?;
    ctx.v1(&format!(
        "Started rebuild of child {} on nexus {}",
        uri, uuid
    ));
    Ok(())
}

async fn stop(
    mut ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    let uuid = matches.value_of("uuid").unwrap().to_string();
    let uri = matches.value_of("uri").unwrap().to_string();

    ctx.v2(&format!(
        "Stopping rebuild of child {} on nexus {}",
        uri, uuid
    ));
    ctx.client
        .stop_rebuild(rpc::StopRebuildRequest {
            uuid: uuid.clone(),
            uri: uri.clone(),
        })
        .await?;
    ctx.v1(&format!(
        "Stopped rebuild of child {} on nexus {}",
        uri, uuid
    ));
    Ok(())
}

async fn pause(
    mut ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    let uuid = matches.value_of("uuid").unwrap().to_string();
    let uri = matches.value_of("uri").unwrap().to_string();

    ctx.v2(&format!(
        "Pausing rebuild of child {} on nexus {}",
        uri, uuid
    ));
    ctx.client
        .pause_rebuild(rpc::PauseRebuildRequest {
            uuid: uuid.clone(),
            uri: uri.clone(),
        })
        .await?;
    ctx.v1(&format!(
        "Paused rebuild of child {} on nexus {}",
        uri, uuid
    ));
    Ok(())
}

async fn resume(
    mut ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    let uuid = matches.value_of("uuid").unwrap().to_string();
    let uri = matches.value_of("uri").unwrap().to_string();

    ctx.v2(&format!(
        "Resuming rebuild of child {} on nexus {}",
        uri, uuid
    ));
    ctx.client
        .resume_rebuild(rpc::ResumeRebuildRequest {
            uuid: uuid.clone(),
            uri: uri.clone(),
        })
        .await?;
    ctx.v1(&format!(
        "Resumed rebuild of child {} on nexus {}",
        uri, uuid
    ));
    Ok(())
}

async fn state(
    mut ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    let uuid = matches.value_of("uuid").unwrap().to_string();
    let uri = matches.value_of("uri").unwrap().to_string();

    ctx.v2(&format!(
        "Getting the rebuild state of child {} on nexus {}",
        uri, uuid
    ));
    let response = ctx
        .client
        .get_rebuild_state(rpc::RebuildStateRequest {
            uuid: uuid.clone(),
            uri: uri.clone(),
        })
        .await?
        .into_inner();
    println!("{}", response.state);
    Ok(())
}

async fn progress(
    mut ctx: Context,
    matches: &ArgMatches<'_>,
) -> Result<(), Status> {
    let uuid = matches.value_of("uuid").unwrap().to_string();
    let uri = matches.value_of("uri").unwrap().to_string();

    ctx.v2(&format!(
        "Getting the rebuild progress of child {} on nexus {}",
        uri, uuid
    ));
    let response = ctx
        .client
        .get_rebuild_progress(rpc::RebuildProgressRequest {
            uuid: uuid.clone(),
            uri: uri.clone(),
        })
        .await?
        .into_inner();
    println!("{}% complete", response.progress);
    Ok(())
}
