// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::bindings::rpcmsg_t;
use crate::bindings::{rpc_ioreq_address_space, rpc_ioreq_direction, rpc_ioreq_len};
use crate::bindings::{RPC_ADDRESS_SPACE_GLOBAL, RPC_MR0_MMIO_DIRECTION_WRITE};
use greens_core::ioreq::{AddressSpace, IoOperation, IoRequest};

impl From<&rpcmsg_t> for IoRequest {
    fn from(ioreq: &rpcmsg_t) -> Self {
        unsafe {
            // FIXME: Safe wrappers
            let address_space = match rpc_ioreq_address_space(ioreq.mr0) {
                RPC_ADDRESS_SPACE_GLOBAL => AddressSpace::Mmio,
                device => AddressSpace::PciConfig {
                    device: device as u8,
                },
            };
            let address = ioreq.mr1;
            let size = rpc_ioreq_len(ioreq.mr0) as u8;

            let operation = match rpc_ioreq_direction(ioreq.mr0) {
                RPC_MR0_MMIO_DIRECTION_WRITE => IoOperation::Write {
                    data: ioreq.mr2.to_ne_bytes(),
                },
                _ => IoOperation::Read,
            };
            IoRequest {
                address_space,
                address,
                size,
                operation,
            }
        }
    }
}
