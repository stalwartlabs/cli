/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

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
