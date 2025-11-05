// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::bar::{PciBar, PciBarIndex};
use crate::configuration_space::PciConfigurationSpace;
use crate::interrupt::PciInterruptSignaler;
use crate::registers::{
    NUM_BAR_REGS, PCI_CACHE_LINE_SIZE, PCI_CLASS_CODE_BASE, PCI_CLASS_CODE_PI, PCI_CLASS_CODE_SUB,
    PCI_DEVICE_ID, PCI_HEADER_TYPE, PCI_HEADER_TYPE_MULTIFUNCTION, PCI_REVISION_ID,
    PCI_SUBSYSTEM_ID, PCI_SUBSYSTEM_VENDOR_ID, PCI_VENDOR_ID,
};
use crate::utils::range_contains;
use crate::{Error, PciMsiMessage, Result};

pub type PciMmioBarOffset = u64;
pub type PciIoBarOffset = u16;

pub trait PciFunction {
    fn read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()>;
    fn write_config(&mut self, offset: usize, data: &[u8]) -> Result<()>;

    fn read_bar(&mut self, _bar_index: PciBarIndex, _offset: u64, _data: &mut [u8]) -> Result<()> {
        Err(Error::NotSupported)
    }

    fn write_bar(&mut self, _bar_index: PciBarIndex, _offset: u64, _data: &[u8]) -> Result<()> {
        Err(Error::NotSupported)
    }

    fn read_mmio(&mut self, address: u64, data: &mut [u8]) -> Result<()> {
        let size = data.len() as u64;
        let (bar, offset) = self
            .find_mmio_region(address, size)
            .ok_or(Error::BarNotFound { address, size })?;

        self.read_bar(bar, offset, data)
    }

    fn write_mmio(&mut self, address: u64, data: &[u8]) -> Result<()> {
        let size = data.len() as u64;
        let (bar, offset) = self
            .find_mmio_region(address, size)
            .ok_or(Error::BarNotFound { address, size })?;
        self.write_bar(bar, offset, data)
    }

    fn read_pio(&mut self, port: u16, data: &mut [u8]) -> Result<()> {
        let (bar, offset) =
            self.find_pio_region(port, data.len() as u16)
                .ok_or(Error::BarNotFound {
                    address: port as u64,
                    size: data.len() as u64,
                })?;
        self.read_bar(bar, offset as u64, data)
    }

    fn write_pio(&mut self, port: u16, data: &[u8]) -> Result<()> {
        let (bar, offset) =
            self.find_pio_region(port, data.len() as u16)
                .ok_or(Error::BarNotFound {
                    address: port as u64,
                    size: data.len() as u64,
                })?;
        self.write_bar(bar, offset as u64, data)
    }

    fn get_bar(&self, index: PciBarIndex) -> Option<PciBar>;

    fn find_mmio_region(&self, address: u64, size: u64) -> Option<(PciBarIndex, PciMmioBarOffset)> {
        find_bar_and_offset(self, address, size, true)
    }

    fn find_pio_region(&self, address: u16, size: u16) -> Option<(PciBarIndex, PciIoBarOffset)> {
        let (index, offset) =
            find_bar_and_offset(self, u64::from(address), u64::from(size), false)?;
        Some((index, offset as u16))
    }
}

pub enum PciHandlerResult<T> {
    Handled(T),
    Unhandled,
}

impl<T> PciHandlerResult<T> {
    pub fn handled(&self) -> bool {
        matches!(self, Self::Handled(_))
    }
}

pub enum PciInterruptConfigEvent {
    MsiMessageUpdate(usize, PciMsiMessage),
    Other,
}

pub trait PciInterruptConfigHandler: PciInterruptSignaler + PciFunctionConfigAccessor {
    fn device_read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()>;
    fn device_write_config(&mut self, offset: usize, data: &[u8]) -> Result<()>;
    fn device_read_bar(&mut self, bar: PciBarIndex, offset: u64, data: &mut [u8]) -> Result<()>;
    fn device_write_bar(&mut self, bar: PciBarIndex, offset: u64, data: &[u8]) -> Result<()>;

    fn read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()> {
        self.preprocess_read_config(offset, data.len())?;
        self.device_read_config(offset, data)
    }

    fn write_config(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        self.device_write_config(offset, data)?;
        if let PciHandlerResult::Handled(r) = self.postprocess_write_config(offset, data.len())? {
            self.on_interrupt_config_write(r);
        }
        Ok(())
    }

    fn read_bar(&mut self, bar: PciBarIndex, offset: u64, data: &mut [u8]) -> Result<()> {
        if let PciHandlerResult::Handled(_) = self.handle_read_bar(bar, offset, data)? {
            return Ok(());
        }

        self.device_read_bar(bar, offset, data)
    }

    fn write_bar(&mut self, bar: PciBarIndex, offset: u64, data: &[u8]) -> Result<()> {
        if let PciHandlerResult::Handled(r) = self.handle_write_bar(bar, offset, data)? {
            self.on_interrupt_config_write(r);
            return Ok(());
        }

        self.device_write_bar(bar, offset, data)
    }

    fn on_interrupt_config_write(&mut self, event: PciInterruptConfigEvent) {
        let _ = event;
    }
}

pub trait PciFunctionConfigAccessor {
    fn config(&self) -> &PciConfigurationSpace;
    fn config_mut(&mut self) -> &mut PciConfigurationSpace;
}

pub trait PciFunctionWithInterrupts: PciInterruptConfigHandler + PciFunctionConfigAccessor {}

// FIXME: Event should cover intx, msi and msix
impl<T> PciFunction for T
where
    T: PciFunctionWithInterrupts,
{
    fn read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()> {
        PciInterruptConfigHandler::read_config(self, offset, data)
    }

    fn write_config(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        PciInterruptConfigHandler::write_config(self, offset, data)
    }

    fn get_bar(&self, index: PciBarIndex) -> Option<crate::bar::PciBar> {
        self.config().get_bar(index)
    }

    fn write_bar(&mut self, bar: PciBarIndex, offset: u64, data: &[u8]) -> Result<()> {
        PciInterruptConfigHandler::write_bar(self, bar, offset, data)
    }

    fn read_bar(&mut self, bar: PciBarIndex, offset: u64, data: &mut [u8]) -> Result<()> {
        PciInterruptConfigHandler::read_bar(self, bar, offset, data)
    }
}

#[derive(Default)]
pub struct PciFunctionConfig {
    pub config: PciConfigurationSpace,
    pub multifunction: bool,
}

impl PciFunctionConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_id(mut self, vendor_id: u16, device_id: u16, rev_id: u8) -> Self {
        // Vendor ID, R/O
        self.config.set_word(PCI_VENDOR_ID, vendor_id);
        // Device ID, R/O
        self.config.set_word(PCI_DEVICE_ID, device_id);
        // Revision ID, R/O
        self.config.set_byte(PCI_REVISION_ID, rev_id);

        self
    }

    pub fn with_class(mut self, baseclass: u8, subclass: u8, programming_interface: u8) -> Self {
        // Class Codes, R/O
        self.config.set_byte(PCI_CLASS_CODE_BASE, baseclass);
        self.config.set_byte(PCI_CLASS_CODE_SUB, subclass);
        self.config
            .set_byte(PCI_CLASS_CODE_PI, programming_interface);

        self
    }

    pub fn with_subsystem_id(mut self, subsystem_vendor_id: u16, subsystem_id: u16) -> Self {
        // Subsystem Vendor ID, R/O
        self.config
            .set_word(PCI_SUBSYSTEM_VENDOR_ID, subsystem_vendor_id);
        // Subsystem ID, R/O
        self.config.set_word(PCI_SUBSYSTEM_ID, subsystem_id);

        self
    }

    pub fn with_header_type(mut self, header_type: u8) -> Self {
        // Header type, R/O
        self.config.set_byte(PCI_HEADER_TYPE, header_type);
        self
    }

    pub fn with_multifunction(mut self) -> Self {
        self.multifunction = true;
        self
    }

    pub fn build(mut self) -> PciConfigurationSpace {
        if self.multifunction {
            let mut header_type = self.config.read_byte(PCI_HEADER_TYPE);
            header_type |= PCI_HEADER_TYPE_MULTIFUNCTION;
            self.config.set_byte(PCI_HEADER_TYPE, header_type);
        }

        // Cache Line Size, implemented as R/W by PCIe devices but has no effect
        self.config.set_writable_byte(PCI_CACHE_LINE_SIZE, 0xFF);

        self.config
    }
}

fn find_bar_and_offset<T: PciFunction + ?Sized>(
    function: &T,
    address: u64,
    size: u64,
    mem: bool,
) -> Option<(PciBarIndex, u64)> {
    for i in 0..NUM_BAR_REGS {
        let index = if let Ok(index) = PciBarIndex::try_from(i) {
            index
        } else {
            continue;
        };

        if let Some(bar_address) = get_matching_bar_address(function, index, address, size, mem) {
            return Some((index, address - bar_address));
        }
    }
    None
}

fn get_matching_bar_address<T: PciFunction + ?Sized>(
    function: &T,
    index: PciBarIndex,
    address: u64,
    size: u64,
    mem: bool,
) -> Option<u64> {
    let bar = function.get_bar(index)?;
    if mem != bar.is_mem() {
        return None;
    }

    let bar_address = bar.address()?;
    if range_contains(bar_address, bar.size(), address, size) {
        return Some(bar_address);
    }
    None
}
