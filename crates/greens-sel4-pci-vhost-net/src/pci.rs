// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::sync::Arc;

use greens_core::io_interface::{InterruptLine, InterruptLineOperation, IoInterface, MsiMessage};
use greens_pci::device::PciDevice;
use greens_pci::function::{PciConfigurationUpdate, PciFunction};
use greens_pci::intx::{PciInterruptLine, PciInterruptLineConfig, PciInterruptLineState};
use greens_pci::{PciInterruptController, PciMsiMessage};
use greens_pci_virtio::pci::VirtioPciFunction;
use greens_sys_linux::eventfd::EventFdBinder;

use crate::vhost_net::VhostNetDevice;

pub(crate) struct InterruptController<T: IoInterface> {
    controller: Arc<T>,
}

impl<T: IoInterface> InterruptController<T> {
    pub(crate) fn new(controller: Arc<T>) -> Self {
        Self { controller }
    }
}

impl<T> PciInterruptController for InterruptController<T>
where
    T: IoInterface,
{
    fn set_interrupt(&mut self, line: PciInterruptLine, state: PciInterruptLineState) {
        let op = match state {
            PciInterruptLineState::Low => InterruptLineOperation::Clear,
            PciInterruptLineState::High => InterruptLineOperation::Set,
        };

        self.controller
            .set_interrupt(line as InterruptLine, op)
            .expect("sending irq failed");
    }

    fn send_msi(&mut self, message: PciMsiMessage) {
        let message = MsiMessage {
            address: message.address,
            data: message.data,
        };

        self.controller
            .send_msi(message)
            .expect("sending msi failed");
    }
}

pub(crate) struct VhostNetPci<T: IoInterface, E: EventFdBinder> {
    function: VirtioPciFunction<InterruptController<T>, VhostNetDevice<E>>,
}

impl<T, E> VhostNetPci<T, E>
where
    T: IoInterface,
    E: EventFdBinder,
{
    pub(crate) fn new(ic: InterruptController<T>, d: VhostNetDevice<E>) -> Self {
        let function = VirtioPciFunction::new(
            Self::pci_device_class(),
            Self::pci_sub_class(),
            PciInterruptLineConfig::Fixed(0),
            ic,
            d,
        )
        .expect("virtio pci function creation failed!");
        Self { function }
    }

    fn pci_device_class() -> u8 {
        // Network device
        2
    }

    pub fn pci_sub_class() -> u8 {
        // Other
        0x80
    }
}

impl<T, E> PciDevice for VhostNetPci<T, E>
where
    T: IoInterface,
    E: EventFdBinder,
{
    fn read_fn_config(
        &mut self,
        function: usize,
        offset: usize,
        data: &mut [u8],
    ) -> greens_pci::Result<()> {
        match function {
            0 => self.function.read_config(offset, data),
            _ => Err(greens_pci::Error::InvalidFunction { function }),
        }
    }

    fn write_fn_config(
        &mut self,
        function: usize,
        offset: usize,
        data: &[u8],
    ) -> greens_pci::Result<Option<PciConfigurationUpdate>> {
        match function {
            0 => self.function.write_config(offset, data),
            _ => Err(greens_pci::Error::InvalidFunction { function }),
        }
    }

    fn read_mmio(&mut self, address: u64, data: &mut [u8]) -> greens_pci::Result<()> {
        self.function.read_mmio(address, data)
    }

    fn write_mmio(
        &mut self,
        address: u64,
        data: &[u8],
    ) -> greens_pci::Result<Option<PciConfigurationUpdate>> {
        self.function.write_mmio(address, data)
    }

    fn read_pio(&mut self, port: u16, data: &mut [u8]) -> greens_pci::Result<()> {
        self.function.read_pio(port, data)
    }

    fn write_pio(
        &mut self,
        port: u16,
        data: &mut [u8],
    ) -> greens_pci::Result<Option<PciConfigurationUpdate>> {
        self.function.write_pio(port, data)
    }
}
