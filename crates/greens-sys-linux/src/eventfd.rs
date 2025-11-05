// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::error;
use std::os::fd;

pub trait EventFdBinder {
    type E: error::Error;

    fn bind_ioeventfd(&self, config: &IoEventFdConfig) -> Result<(), Self::E>;
    fn unbind_ioeventfd(&self, config: &IoEventFdConfig) -> Result<(), Self::E>;
    fn bind_irqfd(&self, config: &IrqFdConfig) -> Result<(), Self::E>;
    fn unbind_irqfd(&self, config: &IrqFdConfig) -> Result<(), Self::E>;
}

#[derive(Debug)]
pub struct IoEventFdConfig {
    pub fd: fd::RawFd,
    pub addr: u64,
    pub len: u32,
    pub data: Option<u64>,
}

impl IoEventFdConfig {
    pub fn new(fd: fd::RawFd, addr: u64, len: u32, data: Option<u64>) -> Self {
        Self {
            fd,
            addr,
            len,
            data,
        }
    }
}

#[derive(Debug)]
pub struct IrqFdConfig {
    pub fd: fd::RawFd,
    pub irq: u32,
}

impl IrqFdConfig {
    pub fn new(fd: fd::RawFd, irq: u32) -> Self {
        Self { fd, irq }
    }
}
