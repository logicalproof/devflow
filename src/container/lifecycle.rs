use std::collections::HashMap;

use bollard::models::HostConfig;
use bollard::query_parameters::{
    BuildImageOptions, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
    StopContainerOptions,
};
use bollard::secret::ContainerCreateBody;
use futures_util::StreamExt;
use http_body_util::Full;
use bytes::Bytes;

use super::docker::DockerClient;
use crate::error::{TreehouseError, Result};

impl DockerClient {
    pub async fn create_and_start_container(
        &self,
        name: &str,
        image: &str,
        workdir: &str,
        bind_mount: &str,
    ) -> Result<String> {
        let options = CreateContainerOptions {
            name: Some(name.to_string()),
            ..Default::default()
        };

        let host_config = HostConfig {
            binds: Some(vec![format!("{bind_mount}:{workdir}")]),
            ..Default::default()
        };

        let config = ContainerCreateBody {
            image: Some(image.to_string()),
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            working_dir: Some(workdir.to_string()),
            host_config: Some(host_config),
            labels: Some(HashMap::from([(
                "managed-by".to_string(),
                "treehouse".to_string(),
            )])),
            ..Default::default()
        };

        let response = self.client.create_container(Some(options), config).await?;

        self.client
            .start_container(&response.id, None::<StartContainerOptions>)
            .await?;

        Ok(response.id)
    }

    pub async fn stop_container(&self, name: &str) -> Result<()> {
        self.client
            .stop_container(name, Some(StopContainerOptions { t: Some(10), signal: None }))
            .await?;
        Ok(())
    }

    pub async fn remove_container(&self, name: &str) -> Result<()> {
        self.client
            .remove_container(
                name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn container_exists(&self, name: &str) -> bool {
        self.client.inspect_container(name, None).await.is_ok()
    }

    pub async fn build_image(&self, dockerfile_content: &str, tag: &str) -> Result<()> {
        // Create a tar archive with the Dockerfile
        let mut header = tar::Header::new_gnu();
        header.set_path("Dockerfile").unwrap();
        header.set_size(dockerfile_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        let mut tar_builder = tar::Builder::new(Vec::new());
        tar_builder
            .append(&header, dockerfile_content.as_bytes())
            .map_err(|e| TreehouseError::Other(format!("Failed to build tar: {e}")))?;
        let tar_bytes = tar_builder
            .into_inner()
            .map_err(|e| TreehouseError::Other(format!("Failed to finalize tar: {e}")))?;

        let options = BuildImageOptions {
            t: Some(tag.to_string()),
            ..Default::default()
        };

        let body = http_body_util::Either::Left(Full::new(Bytes::from(tar_bytes)));
        let mut stream = self.client.build_image(options, None, Some(body));

        while let Some(result) = stream.next().await {
            result?;
        }

        Ok(())
    }
}
