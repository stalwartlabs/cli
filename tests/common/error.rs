use std::io;

use thiserror::Error;

pub type ContainerResult<T> = Result<T, ContainerError>;

#[derive(Debug, Error)]
pub enum ContainerError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("testcontainers: {0}")]
    Testcontainers(#[from] testcontainers::TestcontainersError),
    #[error("reqwest: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("protocol: {0}")]
    Protocol(String),
}
