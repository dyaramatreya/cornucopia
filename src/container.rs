use error::Error;
use std::process::{Command, Stdio};

use self::error::{RemoveContainerError, RunContainerError, StopContainerError};

pub(crate) fn setup(podman: bool) -> Result<(), Error> {
    spawn_container(podman)?;
    healthcheck(podman, 120, 1000)?;
    Ok(())
}

pub(crate) fn cleanup(podman: bool) -> Result<(), Error> {
    stop_container(podman)?;
    remove_container(podman)?;
    Ok(())
}

fn spawn_container(podman: bool) -> Result<(), RunContainerError> {
    let command = if podman { "podman" } else { "docker" };
    let success = Command::new(&command)
        .arg("run")
        .arg("-d")
        .arg("--name")
        .arg("cornucopia_postgres")
        .arg("-p")
        .arg("5432:5432")
        .arg("-e")
        .arg("POSTGRES_PASSWORD=postgres")
        .arg("postgres")
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()?
        .success();

    if success {
        Ok(())
    } else {
        Err(RunContainerError::Status)
    }
}

fn is_postgres_healthy(podman: bool) -> Result<bool, Error> {
    let command = if podman { "podman" } else { "docker" };
    Ok(Command::new(&command)
        .arg("exec")
        .arg("cornucopia_postgres")
        .arg("pg_isready")
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .map_err(Error::HealthCheck)?
        .wait()
        .map_err(Error::HealthCheck)?
        .success())
}

fn healthcheck(podman: bool, max_retries: u64, ms_per_retry: u64) -> Result<(), Error> {
    let mut nb_retries = 0;
    while !is_postgres_healthy(podman)? {
        if nb_retries >= max_retries {
            return Err(Error::MaxNbRetries);
        };
        std::thread::sleep(std::time::Duration::from_millis(ms_per_retry));
        nb_retries += 1;

        if nb_retries % 10 == 0 {
            println!("Container startup slower than expected ({nb_retries} retries out of {max_retries})")
        }
    }
    Ok(())
}

fn stop_container(podman: bool) -> Result<(), StopContainerError> {
    let command = if podman { "podman" } else { "docker" };
    let success = Command::new(&command)
        .arg("stop")
        .arg("cornucopia_postgres")
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()?
        .success();

    if success {
        Ok(())
    } else {
        Err(StopContainerError::Status)
    }
}

fn remove_container(podman: bool) -> Result<(), RemoveContainerError> {
    let command = if podman { "podman" } else { "docker" };
    let success = Command::new(&command)
        .arg("rm")
        .arg("-v")
        .arg("cornucopia_postgres")
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()?
        .success();

    if success {
        Ok(())
    } else {
        Err(RemoveContainerError::Status)
    }
}
pub(crate) mod error {
    use thiserror::Error as ThisError;

    #[derive(Debug, ThisError)]
    pub(crate) enum Error {
        #[error("{0}")]
        RunContainer(#[from] RunContainerError),
        #[error("Encountered error while probing database container health. If you are using `docker`, please check that the daemon is up-and-running. ")]
        HealthCheck(std::io::Error),
        #[error("couldn't stop database container")]
        StopContainer(#[from] StopContainerError),
        #[error("couldn't clean up database container")]
        RemoveContainer(#[from] RemoveContainerError),
        #[error("max number of retries reached while waiting for database container to start")]
        MaxNbRetries,
    }

    #[derive(Debug, ThisError)]
    #[error("Couldn't start database container. If you are using `docker`, please check that the daemon is up-and-running.")]
    pub(crate) enum RunContainerError {
        Io(#[from] std::io::Error),
        #[error("command returned with an error status")]
        Status,
    }

    #[derive(Debug, ThisError)]
    #[error("Couldn't stop database container. If you are using `docker`, please check that the daemon is up-and-running.")]
    pub(crate) enum StopContainerError {
        Io(#[from] std::io::Error),
        #[error("command returned with an error status")]
        Status,
    }

    #[derive(Debug, ThisError)]
    #[error("Couldn't clean up database container. If you are using `docker`, please check that the daemon is up-and-running.")]
    pub(crate) enum RemoveContainerError {
        Io(#[from] std::io::Error),
        #[error("command returned with an error status")]
        Status,
    }
}
