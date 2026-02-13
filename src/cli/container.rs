use clap::Subcommand;
use console::style;

use crate::container::docker::DockerClient;
use crate::error::Result;

#[derive(Subcommand)]
pub enum ContainerCommands {
    /// List containers managed by devflow
    List,
    /// Build a container image
    Build {
        /// Task or container name
        name: String,
    },
    /// Start a container
    Start {
        /// Task or container name
        name: String,
    },
    /// Stop a container
    Stop {
        /// Task or container name
        name: String,
    },
    /// Open a shell in a running container
    Shell {
        /// Task or container name
        name: String,
    },
}

pub async fn run(cmd: ContainerCommands) -> Result<()> {
    match cmd {
        ContainerCommands::List => list().await,
        ContainerCommands::Build { name } => build(&name).await,
        ContainerCommands::Start { name } => start(&name).await,
        ContainerCommands::Stop { name } => stop(&name).await,
        ContainerCommands::Shell { name } => shell(&name).await,
    }
}

async fn list() -> Result<()> {
    let docker = DockerClient::connect().await?;

    use bollard::query_parameters::ListContainersOptions;
    use std::collections::HashMap;

    let filters = HashMap::from([("label".to_string(), vec!["managed-by=devflow".to_string()])]);
    let options = ListContainersOptions {
        all: true,
        filters: Some(filters),
        ..Default::default()
    };

    let containers = docker.client.list_containers(Some(options)).await?;

    if containers.is_empty() {
        println!("No devflow containers found.");
        return Ok(());
    }

    println!("{}", style("Devflow containers:").bold());
    for c in &containers {
        let name = c
            .names
            .as_ref()
            .and_then(|n| n.first())
            .map(|n| n.trim_start_matches('/'))
            .unwrap_or("unknown");
        let state = c.state.as_ref().map(|s| format!("{s:?}")).unwrap_or_else(|| "unknown".to_string());
        let image = c.image.as_deref().unwrap_or("unknown");

        let state_styled = if state.contains("Running") {
            style(&state).green()
        } else if state.contains("Exited") {
            style(&state).red()
        } else {
            style(&state).yellow()
        };

        println!("  {} {} [{}] image:{}", style("●").cyan(), name, state_styled, image);
    }

    Ok(())
}

async fn build(name: &str) -> Result<()> {
    let docker = DockerClient::connect().await?;

    let tag = format!("devflow-{name}:latest");
    println!("Building image '{tag}'...");

    // Use a simple base image for now
    let dockerfile = "FROM ubuntu:22.04\nRUN apt-get update -qq\nCMD [\"sleep\", \"infinity\"]\n";
    docker.build_image(dockerfile, &tag).await?;

    println!("{} Image '{}' built", style("✓").green().bold(), tag);
    Ok(())
}

async fn start(name: &str) -> Result<()> {
    let docker = DockerClient::connect().await?;

    let container_name = format!("devflow-{name}");
    let image = format!("devflow-{name}:latest");

    if docker.container_exists(&container_name).await {
        println!("Container '{container_name}' already exists. Stopping first...");
        let _ = docker.stop_container(&container_name).await;
        docker.remove_container(&container_name).await?;
    }

    let id = docker
        .create_and_start_container(&container_name, &image, "/app", ".")
        .await?;

    println!(
        "{} Container '{}' started ({})",
        style("✓").green().bold(),
        container_name,
        &id[..12]
    );
    Ok(())
}

async fn stop(name: &str) -> Result<()> {
    let docker = DockerClient::connect().await?;
    let container_name = format!("devflow-{name}");

    docker.stop_container(&container_name).await?;
    docker.remove_container(&container_name).await?;

    println!(
        "{} Container '{}' stopped and removed",
        style("✓").green().bold(),
        container_name
    );
    Ok(())
}

async fn shell(name: &str) -> Result<()> {
    let container_name = format!("devflow-{name}");

    // Shell out to docker exec for interactive terminal
    let status = std::process::Command::new("docker")
        .args(["exec", "-it", &container_name, "/bin/bash"])
        .status()?;

    if !status.success() {
        // Try sh as fallback
        let status = std::process::Command::new("docker")
            .args(["exec", "-it", &container_name, "/bin/sh"])
            .status()?;

        if !status.success() {
            return Err(crate::error::DevflowError::ContainerNotFound(
                container_name,
            ));
        }
    }

    Ok(())
}
