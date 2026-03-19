// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use greens_core::io_interface::IoInterface;
use greens_core::ioreq::AddressSpace::{Mmio, PciConfig};
use greens_core::ioreq::IoOperation;
use greens_pci::device::PciDevice;
use greens_sys_linux::eventfd::EventFdBinder;
use vm_memory::GuestMemoryMmap;

use crate::vhost_net::VhostNetConfig;
use crate::{InterruptController, VhostNetDevice, VhostNetPci};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    // FIXME: we need to propagate the error from handler
    #[error("failed to handle i/o request")]
    HandleIoRequest,
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct DeviceModel<I: IoInterface, B: EventFdBinder> {
    running: Arc<AtomicBool>,
    io_interface: Arc<I>,
    device: VhostNetPci<I, B>,
}

impl<I: IoInterface, B: EventFdBinder> DeviceModel<I, B> {
    pub fn new(
        io_interface: Arc<I>,
        binder: Arc<B>,
        guest_memory: GuestMemoryMmap,
        config: VhostNetConfig,
    ) -> Self {
        let controller = InterruptController::new(io_interface.clone());
        let vhost = VhostNetDevice::new(binder, guest_memory, config);

        let device = VhostNetPci::new(controller, vhost);

        Self {
            running: Arc::new(AtomicBool::new(true)),
            io_interface,
            device,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        while self.running.load(Ordering::Relaxed) {
            self.io_interface
                .handle_next_io(&mut |request| -> Option<[u8; 8]> {
                    match request.address_space {
                        PciConfig { device: pcidev } => match request.operation {
                            IoOperation::Read => {
                                let mut output = [0xFFu8; 8];
                                match pcidev {
                                    0 => self
                                        .device
                                        .read_config_le(
                                            request.address as usize,
                                            &mut output[0..request.size as usize],
                                        )
                                        // FIXME
                                        .unwrap(),
                                    d => eprintln!("read to unknown pci device {d}"),
                                }
                                Some(output)
                            }
                            IoOperation::Write { data: input } => {
                                match pcidev {
                                    0 => {
                                        // FIXME
                                        self.device
                                            .write_config_le(
                                                request.address as usize,
                                                &input[0..request.size as usize],
                                            )
                                            .unwrap();
                                    }
                                    d => eprintln!("write to unknown pci device {d}"),
                                }
                                None
                            }
                        },
                        Mmio => match request.operation {
                            IoOperation::Read => {
                                let mut output = [0xFFu8; 8];
                                self.device
                                    .read_mmio(
                                        request.address,
                                        &mut output[0..request.size as usize],
                                    )
                                    .unwrap();
                                Some(output)
                            }
                            IoOperation::Write { data: input } => {
                                self.device
                                    .write_mmio(request.address, &input[0..request.size as usize])
                                    // FIXME
                                    .unwrap();
                                None
                            }
                        },
                    }
                })
                .map_err(|_e| Error::HandleIoRequest)?;
        }
        Ok(())
    }
}
