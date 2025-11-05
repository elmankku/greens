// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
#[derive(Debug, PartialEq, Eq)]
pub enum IoOperation {
    Read,
    Write { data: [u8; 8] },
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddressSpace {
    Mmio,
    PciConfig { device: u8 },
}

#[derive(Debug, PartialEq, Eq)]
pub struct IoRequest {
    pub address_space: AddressSpace,
    pub address: u64,
    pub size: u8,
    pub operation: IoOperation,
}
