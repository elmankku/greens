// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::fmt;

#[derive(Debug, PartialEq)]
pub struct MsiMessage {
    pub address: u64,
    pub data: u32,
}

use crate::ioreq::IoRequest;

pub type InterruptLine = u32;

#[derive(Debug)]
pub enum InterruptLineOperation {
    Clear,
    Set,
    Pulse,
}

pub trait IoInterface {
    type E: fmt::Debug;

    fn handle_next_io(
        &self,
        io_handle_fn: &mut dyn FnMut(IoRequest) -> Option<[u8; 8]>,
    ) -> Result<(), Self::E>;

    fn set_interrupt(&self, irq: InterruptLine, op: InterruptLineOperation) -> Result<(), Self::E>;
    fn send_msi(&self, message: MsiMessage) -> Result<(), Self::E>;
}
