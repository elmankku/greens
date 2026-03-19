// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::bar::{PciBar, PciBarIndex};
use crate::configuration_space::PciConfigurationSpace;
use crate::interrupt::{PciInterrupt, PciInterruptHandler, PciInterruptType};
use crate::registers::{
    NUM_BAR_REGS, PCI_CACHE_LINE_SIZE, PCI_CLASS_CODE_BASE, PCI_CLASS_CODE_PI, PCI_CLASS_CODE_SUB,
    PCI_COMMAND, PCI_COMMAND_IO_SPACE_MASK, PCI_COMMAND_MEM_SPACE_MASK, PCI_DEVICE_ID,
    PCI_HEADER_TYPE, PCI_HEADER_TYPE_MULTIFUNCTION, PCI_REVISION_ID, PCI_SUBSYSTEM_ID,
    PCI_SUBSYSTEM_VENDOR_ID, PCI_VENDOR_ID,
};
use crate::utils::range_contains;
use crate::{Error, PciInterruptController, PciMsiMessage, Result};

pub type PciMmioBarOffset = u64;
pub type PciIoBarOffset = u16;

#[derive(Debug, Clone, PartialEq)]
pub enum PciConfigurationUpdate {
    MsiMessage(PciMsiMessage),
    MsiXMessage(usize, PciMsiMessage),
    /// BAR update.
    Bar(PciBar),
    /// Indicates MEM_SPACE and IO_SPACE bit changes, which connect/disconnect BARs from address
    /// space.
    SpaceChanged,
}

pub trait PciFunction {
    fn read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()>;
    fn write_config(
        &mut self,
        offset: usize,
        data: &[u8],
    ) -> Result<Option<PciConfigurationUpdate>>;

    fn read_bar(&mut self, bar_index: PciBarIndex, offset: u64, data: &mut [u8]) -> Result<()> {
        let _ = (bar_index, offset, data);
        Err(Error::NotSupported)
    }

    fn write_bar(
        &mut self,
        bar_index: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<Option<PciConfigurationUpdate>> {
        let _ = (bar_index, offset, data);
        Err(Error::NotSupported)
    }

    fn read_mmio(&mut self, address: u64, data: &mut [u8]) -> Result<()> {
        let size = data.len() as u64;
        let (bar, offset) = self
            .find_mmio_region(address, size)
            .ok_or(Error::BarNotFound { address, size })?;

        self.read_bar(bar, offset, data)
    }

    fn write_mmio(&mut self, address: u64, data: &[u8]) -> Result<Option<PciConfigurationUpdate>> {
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

    fn write_pio(&mut self, port: u16, data: &[u8]) -> Result<Option<PciConfigurationUpdate>> {
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

#[derive(Debug, PartialEq)]
pub enum PciHandlerResult<T> {
    Handled(T),
    Unhandled,
}

impl<T> PciHandlerResult<T> {
    pub fn handled(&self) -> bool {
        matches!(self, Self::Handled(_))
    }
}

pub trait PciFunctionConfigAccessor {
    fn config(&self) -> &PciConfigurationSpace;
    fn config_mut(&mut self) -> &mut PciConfigurationSpace;
}

/// Trait for the device-specific side of a PCI function. The device owns the
/// `PciConfigurationSpace` (via [`PciFunctionConfigAccessor`]) and is responsible
/// for all BAR dispatch. [`PciFunctionWithInterrupts`] owns the interrupt mechanism
/// and controller, composing them with the device.
pub trait PciDeviceHandler: PciFunctionConfigAccessor {
    /// Called after a config space write for device-specific post-processing.
    fn on_write_config(&mut self, offset: usize, size: usize) -> Result<()> {
        let _ = (offset, size);
        Ok(())
    }

    /// Called before a config space read to allow the device to refresh live values.
    fn prepare_read_config(&mut self, offset: usize, size: usize) {
        let _ = (offset, size);
    }

    fn read_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
        irq: &mut PciInterruptAccess<'_>,
    ) -> Result<()> {
        let _ = (bar, offset, data, irq);
        Err(Error::NotSupported)
    }

    fn write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
        irq: &mut PciInterruptAccess<'_>,
    ) -> Result<()> {
        let _ = (bar, offset, data, irq);
        Err(Error::NotSupported)
    }

    /// Called when a notable configuration space change occurs that the device must track,
    /// such as an MSI message update or a BAR address change.
    fn on_config_update(&mut self, event: PciConfigurationUpdate) {
        let _ = event;
    }
}

/// Provides the device handler with interrupt context during BAR accesses:
/// the currently active interrupt type, and the ability to signal an interrupt.
pub struct PciInterruptAccess<'a> {
    active: PciInterruptType,
    signal: &'a mut dyn FnMut(&mut PciConfigurationSpace, PciInterrupt) -> Result<()>,
}

impl<'a> PciInterruptAccess<'a> {
    fn new(
        active: PciInterruptType,
        signal: &'a mut dyn FnMut(&mut PciConfigurationSpace, PciInterrupt) -> Result<()>,
    ) -> Self {
        Self { active, signal }
    }

    pub fn active(&self) -> PciInterruptType {
        self.active
    }

    pub fn signal(&mut self, config: &mut PciConfigurationSpace, irq: PciInterrupt) -> Result<()> {
        (self.signal)(config, irq)
    }
}

/// Concrete wrapper that composes a device handler `D` (which owns the config
/// space) with an interrupt mechanism `I` and interrupt controller `C`.
pub struct PciFunctionWithInterrupts<C, D, I>
where
    C: PciInterruptController,
    I: PciInterruptHandler<C>,
{
    controller: C,
    device: D,
    interrupt: I,
}

impl<C, D, I> PciFunctionWithInterrupts<C, D, I>
where
    C: PciInterruptController,
    I: PciInterruptHandler<C>,
{
    pub fn new(controller: C, device: D, interrupt: I) -> Self {
        Self {
            controller,
            device,
            interrupt,
        }
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    pub fn controller(&self) -> &C {
        &self.controller
    }
}

impl<C, D, I> PciFunctionConfigAccessor for PciFunctionWithInterrupts<C, D, I>
where
    C: PciInterruptController,
    D: PciFunctionConfigAccessor,
    I: PciInterruptHandler<C>,
{
    fn config(&self) -> &PciConfigurationSpace {
        self.device.config()
    }

    fn config_mut(&mut self) -> &mut PciConfigurationSpace {
        self.device.config_mut()
    }
}

impl<C, D, I> PciFunctionWithInterrupts<C, D, I>
where
    C: PciInterruptController,
    D: PciDeviceHandler,
    I: PciInterruptHandler<C>,
{
    pub fn active_interrupt(&self) -> PciInterruptType {
        self.interrupt.active_interrupt(self.device.config())
    }

    pub fn signal_interrupt(&mut self, interrupt: PciInterrupt) -> Result<()> {
        if self.interrupt.is_enabled(self.device.config()) {
            self.interrupt
                .signal(self.device.config_mut(), &mut self.controller, interrupt)
        } else {
            Ok(())
        }
    }
}

impl<C, D, I> PciFunction for PciFunctionWithInterrupts<C, D, I>
where
    C: PciInterruptController,
    D: PciDeviceHandler,
    I: PciInterruptHandler<C>,
{
    fn read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()> {
        self.device.prepare_read_config(offset, data.len());
        self.device.config_mut().read_checked(offset, data)
    }

    fn write_config(
        &mut self,
        offset: usize,
        data: &[u8],
    ) -> Result<Option<PciConfigurationUpdate>> {
        // Capture previous command register value.
        let prev_command = self.device.config().read_word(PCI_COMMAND);

        // Write to configuration space.
        self.device.config_mut().write_checked(offset, data)?;

        // Device write hook.
        self.device.on_write_config(offset, data.len())?;

        // Handle changes to interrupts.
        if let PciHandlerResult::Handled(Some(event)) = self.interrupt.on_write_config(
            self.device.config_mut(),
            &mut self.controller,
            offset,
            data.len(),
        )? {
            self.device.on_config_update(event.clone());
            return Ok(Some(event));
        }

        // Handle updates to BARs.
        if let Some(bar) = self.device.config().bar_update_for_write(offset) {
            let event = PciConfigurationUpdate::Bar(bar);
            self.device.on_config_update(event.clone());
            return Ok(Some(event));
        }

        // Handle changes to IO_SPACE/MMIO_SPACE bits.
        let new_command = self.device.config().read_word(PCI_COMMAND);
        let space_mask = PCI_COMMAND_IO_SPACE_MASK | PCI_COMMAND_MEM_SPACE_MASK;
        if (prev_command ^ new_command) & space_mask != 0 {
            self.device
                .on_config_update(PciConfigurationUpdate::SpaceChanged);
            return Ok(Some(PciConfigurationUpdate::SpaceChanged));
        }

        Ok(None)
    }

    fn get_bar(&self, index: PciBarIndex) -> Option<PciBar> {
        self.device.config().get_bar(index)
    }

    fn read_bar(&mut self, bar: PciBarIndex, offset: u64, data: &mut [u8]) -> Result<()> {
        if self
            .interrupt
            .read_bar(
                self.device.config_mut(),
                &mut self.controller,
                bar,
                offset,
                data,
            )?
            .handled()
        {
            return Ok(());
        }
        let active = self.interrupt.active_interrupt(self.device.config());
        let controller = &mut self.controller;
        let interrupt = &mut self.interrupt;
        let mut signal_fn = |config: &mut PciConfigurationSpace, irq: PciInterrupt| {
            if interrupt.is_enabled(config) {
                interrupt.signal(config, controller, irq)
            } else {
                Ok(())
            }
        };
        let mut irq = PciInterruptAccess::new(active, &mut signal_fn);
        self.device.read_bar(bar, offset, data, &mut irq)
    }

    fn write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<Option<PciConfigurationUpdate>> {
        if let PciHandlerResult::Handled(event) = self.interrupt.write_bar(
            self.device.config_mut(),
            &mut self.controller,
            bar,
            offset,
            data,
        )? {
            if let Some(e) = event {
                self.device.on_config_update(e.clone());
                return Ok(Some(e));
            }

            return Ok(None);
        }

        let active = self.interrupt.active_interrupt(self.device.config());
        let controller = &mut self.controller;
        let interrupt = &mut self.interrupt;
        let mut signal_fn = |config: &mut PciConfigurationSpace, irq: PciInterrupt| {
            if interrupt.is_enabled(config) {
                interrupt.signal(config, controller, irq)
            } else {
                Ok(())
            }
        };
        let mut irq = PciInterruptAccess::new(active, &mut signal_fn);
        self.device.write_bar(bar, offset, data, &mut irq)?;

        Ok(None)
    }
}

#[derive(Default)]
pub struct PciFunctionBuilder {
    pub config: PciConfigurationSpace,
    pub multifunction: bool,
}

impl PciFunctionBuilder {
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
