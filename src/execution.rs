use std::{
    fs::{self, read_to_string, write},
    io::BufRead,
    os::unix::fs::PermissionsExt,
    time::Duration,
};

use chrono::{Timelike, Utc};
use ipipe::Pipe;
use serde::Serialize;
use tokio::{sync::mpsc::Sender, time::sleep};

use crate::{
    GenResult, create_path, email::send_welcome_mail, errors::ResultLog, get_instance,
    ical::get_ical_path, kuma,
};

type StartMinute = u8;

#[derive(PartialEq, Serialize)]
pub enum StartReason {
    Direct,
    Timer,
    Single,
    Pipe,
    Force,
}

fn get_execution_properties() -> (Duration, StartMinute) {
    let (user, _properties) = get_instance();
    let cycle_time = (user.user_properties.execution_interval_minutes * 60) as u64;
    let starting_minute = || -> GenResult<u8> {
        let path = create_path("starting_minute");
        let starting_minute_str =
            read_to_string(&path).unwrap_or(rand::random_range(0..60).to_string());
        _ = write(&path, starting_minute_str.as_bytes());
        Ok(starting_minute_str.parse()?)
    }()
    .unwrap_or(rand::random_range(0..60));
    (Duration::from_secs(cycle_time), starting_minute)
}

pub async fn execution_manager(tx: Sender<StartReason>, instant_run: bool) {
    let execution_properties = get_execution_properties();
    let current_time = Utc::now();
    if instant_run {
        _ = tx.send(StartReason::Direct).await;
    }
    if current_time.minute() != execution_properties.1 as u32 {
        let current_minute = current_time.minute() as i32;
        let mut waiting_minutes = execution_properties.1 as i32 - current_minute;
        let exectution_hour_interval = execution_properties.0.as_secs() / 3600;
        // Pick how many hours it should randomly wait so that the site is not bombarded with requests for users who all run less than every hour
        let hour_randomization = rand::random_range(0..exectution_hour_interval) as i32;
        waiting_minutes += hour_randomization * 60;
        if waiting_minutes < 0 {
            waiting_minutes += 60;
        }
        debug!("Waiting {waiting_minutes} minutes until execution");
        sleep(Duration::from_secs(waiting_minutes as u64 * 60)).await;
    }

    loop {
        info!("Starting execution loop");
        _ = tx.try_send(StartReason::Timer);

        sleep(execution_properties.0).await;
    }
}

pub async fn start_pipe(tx: Sender<StartReason>) -> GenResult<()> {
    let (user, _properties) = get_instance();
    let pipe_path = create_path("pipe");
    if pipe_path.exists() {
        info!("Previous pipe file found, removing");
        std::fs::remove_file(&pipe_path).warn("Removing previous pipe");
    }
    let pipe = Pipe::open(&pipe_path, ipipe::OnCleanup::Delete).unwrap_or(Pipe::with_name("pipe")?);
    if let Ok(metadata) = fs::metadata(&pipe_path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o666);
        fs::set_permissions(&pipe_path, permissions).info("Setting permissions");
    } else {
        warn!("Failed to set permissions of pipe");
    }

    let reader = std::io::BufReader::new(pipe);
    // Depending on what message is sent from the pipe, changes the behaviour of the program
    // q: Quit webcom ical
    // f: Force the execution, ignoring incorrect credentials
    // w: Send a welcome mail
    // k: Run kuma logic
    // p: Reload env variables and print the set password to the log
    for line in reader.lines() {
        let start_reason = match &line {
            Ok(line) if line == "q" => return Ok(()),
            Ok(line) if line == "f" => Some(StartReason::Force),
            Ok(line) if line == "w" => {
                let ical_path = get_ical_path().warn_owned("Getting ical path");
                if let Ok(ical_path) = ical_path {
                    send_welcome_mail(&ical_path, true).warn("Sending welcome mail");
                }
                None
            }
            Ok(line) if line == "k" => {
                debug!("Checking if kuma needs to be created");
                kuma::first_run().await.warn("Kuma Run in pipe");
                None
            }
            Ok(line) if line == "p" => {
                error!("Password is: {}", user.password);
                None
            }
            Ok(_) => Some(StartReason::Pipe),
            _ => None,
        };
        debug!(
            "Recieved message from pipe: {}",
            line.unwrap_or("Error".to_owned())
        );
        _ = start_reason.is_some_and(|start_reason| {
            tx.try_send(start_reason)
                .info("Send start request from pipe");
            false
        });
    }
    Ok(())
}
