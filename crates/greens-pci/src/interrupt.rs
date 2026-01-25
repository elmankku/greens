// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::bar::PciBarIndex;
use crate::configuration_space::PciConfigurationSpace;
use crate::function::{PciConfigurationUpdate, PciHandlerResult};
use crate::intx;
use crate::intx::{PciInterruptLineState, PciIntx};
use crate::msi::{PciMsiGenerationResult, PciMsiMessageSource, PciMsiVector};
use crate::{Error, PciInterruptController, Result};

pub enum PciInterruptType {
    Intx,
    Msi,
    MsiX,
    NoInterrupt,
}

pub enum PciInterrupt {
    Intx(PciInterruptLineState),
    Msi(PciMsiVector),
}

pub trait PciInterruptSignaler {
    fn signal_interrupt(&mut self, interrupt: PciInterrupt) -> Result<()>;

    fn active_interrupt(&self) -> PciInterruptType;

    fn preprocess_read_config(
        &mut self,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let _ = offset;
        let _ = size;

        Ok(PciHandlerResult::Unhandled)
    }
    fn postprocess_write_config(
        &mut self,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let _ = offset;
        let _ = size;

        Ok(PciHandlerResult::Unhandled)
    }

    fn handle_read_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let _ = bar;
        let _ = offset;
        let _ = data;

        Ok(PciHandlerResult::Unhandled)
    }

    fn handle_write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let _ = bar;
        let _ = offset;
        let _ = data;

        Ok(PciHandlerResult::Unhandled)
    }
}

pub fn handle_msi(
    config: &mut PciConfigurationSpace,
    interrupt_controller: &mut impl PciInterruptController,
    source: &mut impl PciMsiMessageSource,
    interrupt: PciInterrupt,
) -> Result<()> {
    match interrupt {
        PciInterrupt::Msi(vector) => {
            handle_msi_generation(
                source.try_generate_message(config, vector)?,
                interrupt_controller,
            );
            Ok(())
        }
        PciInterrupt::Intx(_) => Err(Error::InterruptTypeMismatch),
    }
}

pub fn handle_intx(
    config: &mut PciConfigurationSpace,
    interrupt_controller: &mut impl PciInterruptController,
    intx: &PciIntx,
    interrupt: PciInterrupt,
) -> Result<()> {
    match interrupt {
        PciInterrupt::Intx(level) => {
            intx::signal_intx(config, interrupt_controller, intx, level);
            Ok(())
        }
        PciInterrupt::Msi(_) => Err(Error::InterruptTypeMismatch),
    }
}

fn handle_msi_generation(
    result: PciMsiGenerationResult,
    interrupt_controller: &mut impl PciInterruptController,
) {
    if let PciMsiGenerationResult::Generated(message) = result {
        interrupt_controller.send_msi(message);
    }
}
