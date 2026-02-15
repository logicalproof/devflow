use bollard::Docker;

use crate::error::{TreehouseError, Result};

pub struct DockerClient {
    pub client: Docker,
}

impl DockerClient {
    pub async fn connect() -> Result<Self> {
        let client =
            Docker::connect_with_local_defaults().map_err(|_| TreehouseError::DockerNotAvailable)?;

        // Verify connection
        client
            .ping()
            .await
            .map_err(|_| TreehouseError::DockerNotAvailable)?;

        Ok(Self { client })
    }
}
