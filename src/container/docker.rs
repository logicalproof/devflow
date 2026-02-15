use bollard::Docker;

use crate::error::{GrootError, Result};

pub struct DockerClient {
    pub client: Docker,
}

impl DockerClient {
    pub async fn connect() -> Result<Self> {
        let client =
            Docker::connect_with_local_defaults().map_err(|_| GrootError::DockerNotAvailable)?;

        // Verify connection
        client
            .ping()
            .await
            .map_err(|_| GrootError::DockerNotAvailable)?;

        Ok(Self { client })
    }
}
