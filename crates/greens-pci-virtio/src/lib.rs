// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::Error as PciError;

pub mod pci;
pub mod pci_cap;
pub mod pci_cfg_cap;
pub mod pci_common_cfg;
pub mod pci_device_cfg;
pub mod pci_isr_cfg;
pub mod pci_notify_cfg;

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum Error {
    #[error("pci error: {0}")]
    PciError(#[from] PciError),
}
pub type Result<T> = core::result::Result<T, Error>;
