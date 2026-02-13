use bollard::Docker;

use crate::error::{DevflowError, Result};

pub struct DockerClient {
    pub client: Docker,
}

impl DockerClient {
    pub async fn connect() -> Result<Self> {
        let client =
            Docker::connect_with_local_defaults().map_err(|_| DevflowError::DockerNotAvailable)?;

        // Verify connection
        client
            .ping()
            .await
            .map_err(|_| DevflowError::DockerNotAvailable)?;

        Ok(Self { client })
    }
}
