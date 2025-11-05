// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_sel4::io_interface::Error as IoInterfaceError;

#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub enum Error {
    #[error("failed to initialize driver interface: {0}")]
    Init(IoInterfaceError),
    #[error("tap interface name too long: {len} (max: {max})")]
    TapNameTooLong { len: usize, max: usize },
    #[error("failed to configure tap device: {0}")]
    TapConfiguration(String),
}

pub type Result<T> = std::result::Result<T, Error>;
