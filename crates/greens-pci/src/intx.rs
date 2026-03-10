// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::PciInterruptController;
use crate::configuration_space::PciConfigurationSpace;
use crate::registers::{
    PCI_COMMAND, PCI_COMMAND_INTERRUPT_DISABLE_MASK, PCI_INTERRUPT_LINE, PCI_INTERRUPT_PIN,
    PCI_STATUS, PCI_STATUS_INTERRUPT_STATUS_MASK,
};
use crate::utils::range_overlaps;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PciInterruptLineState {
    Low = 0,
    High = 1,
}

impl From<bool> for PciInterruptLineState {
    fn from(state: bool) -> Self {
        match state {
            true => PciInterruptLineState::High,
            false => PciInterruptLineState::Low,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PciIntxPin {
    IntA = 1,
    IntB = 2,
    IntC = 3,
    IntD = 4,
}

impl From<PciIntxPin> for u8 {
    fn from(pin: PciIntxPin) -> Self {
        pin as u8
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PciInterruptLineConfig {
    Register,
    Fixed(PciInterruptLine),
}

pub type PciInterruptLine = usize;

pub struct PciIntxConfig {
    pin: PciIntxPin,
    line_config: PciInterruptLineConfig,
}

impl PciIntxConfig {
    pub fn new(pin: PciIntxPin, line_config: PciInterruptLineConfig) -> Self {
        Self { pin, line_config }
    }

    pub fn build(&self, config: &mut PciConfigurationSpace) -> PciIntx {
        init_intx(config, self.pin);

        PciIntx {
            line_config: self.line_config,
            disabled: true,
        }
    }
}

pub struct PciIntx {
    line_config: PciInterruptLineConfig,
    disabled: bool,
}

pub fn init_intx(config: &mut PciConfigurationSpace, pin: PciIntxPin) {
    // Set PIN information.
    config.set_byte(PCI_INTERRUPT_PIN, pin.into());

    // Add Interrupt Disable to writable bits.
    config.add_writable_word(PCI_COMMAND, PCI_COMMAND_INTERRUPT_DISABLE_MASK);

    // Interrupt Line register writable; used by FW to convey interrupt line to OS.
    config.set_writable_byte(PCI_INTERRUPT_LINE, 0xFF);
}

pub fn interrupts_disabled(config: &PciConfigurationSpace) -> bool {
    let command = config.read_word(PCI_COMMAND);

    command & PCI_COMMAND_INTERRUPT_DISABLE_MASK != 0
}

pub fn interrupt_pending(config: &PciConfigurationSpace) -> bool {
    let status = config.read_word(PCI_STATUS);

    status & PCI_STATUS_INTERRUPT_STATUS_MASK != 0
}

pub fn interrupt_line(
    config: &PciConfigurationSpace,
    line_config: PciInterruptLineConfig,
) -> PciInterruptLine {
    match line_config {
        PciInterruptLineConfig::Fixed(l) => l,
        PciInterruptLineConfig::Register => config.read_byte(PCI_INTERRUPT_LINE).into(),
    }
}

pub fn update_interrupt_status(
    config: &mut PciConfigurationSpace,
    level: PciInterruptLineState,
) -> bool {
    let current = config.read_word(PCI_STATUS);

    let new = match level {
        PciInterruptLineState::Low => current & !PCI_STATUS_INTERRUPT_STATUS_MASK,
        PciInterruptLineState::High => current | PCI_STATUS_INTERRUPT_STATUS_MASK,
    };

    config.set_word(PCI_STATUS, new);

    current != new
}

pub fn signal_intx(
    config: &mut PciConfigurationSpace,
    interrupt_controller: &mut impl PciInterruptController,
    intx: &PciIntx,
    level: PciInterruptLineState,
) {
    // Update status register to reflect the new level.
    let changed = update_interrupt_status(config, level);

    // Update the interrupt line level only if interrupts are enabled and the level was changed.
    if !interrupts_disabled(config) && changed {
        let line = interrupt_line(config, intx.line_config);

        interrupt_controller.set_interrupt(line, level);
    }
}

pub fn postprocess_write_config(
    config: &mut PciConfigurationSpace,
    interrupt_controller: &mut impl PciInterruptController,
    intx: &mut PciIntx,
    offset: usize,
    size: usize,
) {
    if !range_overlaps(PCI_COMMAND, 2, offset, size) {
        return;
    }

    let disabled = interrupts_disabled(config);

    // Handle intx enable/disable.
    if disabled != intx.disabled && interrupt_pending(config) {
        let line = interrupt_line(config, intx.line_config);

        // Update cached value
        intx.disabled = disabled;

        if disabled {
            // Disabled. Force INTx low.
            interrupt_controller.set_interrupt(line, PciInterruptLineState::Low);
        } else {
            interrupt_controller.set_interrupt(line, PciInterruptLineState::High)
        }
    }
}
