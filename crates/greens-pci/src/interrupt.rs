// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::bar::PciBarIndex;
use crate::bar_region::PciBarRegionSetHandler;
use crate::config_handler::PciConfigurationSpaceIoHandler;
use crate::configuration_space::PciConfigurationSpace;
use crate::function::{PciConfigurationUpdate, PciHandlerResult};
use crate::intx;
use crate::intx::{PciInterruptLineState, PciIntx};
use crate::msi::{PciMsi, PciMsiGenerationResult, PciMsiMessageSource, PciMsiVector};
use crate::msix::{MsiXBarHandlerContext, MsiXTable, PbaTable, PciMsiX};
use crate::{Error, PciInterruptController, Result};

#[derive(Copy, Clone, PartialEq)]
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

pub trait InterruptSignaler<C: PciInterruptController> {
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        let _ = config;
        false
    }

    fn active_interrupt(&self, config: &PciConfigurationSpace) -> PciInterruptType {
        let _ = config;
        PciInterruptType::NoInterrupt
    }

    fn signal(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        interrupt: PciInterrupt,
    ) -> Result<()> {
        let _ = (config, controller, interrupt);
        Ok(())
    }
}

pub trait InterruptConfigHandler<C: PciInterruptController> {
    fn on_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let _ = (config, controller, offset, size);
        Ok(PciHandlerResult::Unhandled)
    }
}

pub trait InterruptBarHandler<C: PciInterruptController> {
    fn read_bar(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
    ) -> Result<PciHandlerResult<()>> {
        let _ = (config, controller, bar, offset, data);
        Ok(PciHandlerResult::Unhandled)
    }

    fn write_bar(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let _ = (config, controller, bar, offset, data);
        Ok(PciHandlerResult::Unhandled)
    }
}

pub trait IntxHandler<C: PciInterruptController>:
    InterruptSignaler<C> + InterruptConfigHandler<C>
{
}

pub trait MsiHandler<C: PciInterruptController>:
    InterruptSignaler<C> + InterruptConfigHandler<C>
{
}

pub trait MsiXHandler<C: PciInterruptController>:
    InterruptSignaler<C> + InterruptConfigHandler<C> + InterruptBarHandler<C>
{
}

pub trait PciInterruptHandler<C: PciInterruptController>:
    InterruptSignaler<C> + InterruptConfigHandler<C> + InterruptBarHandler<C>
{
}

impl<C, T> PciInterruptHandler<C> for T
where
    C: PciInterruptController,
    T: InterruptSignaler<C> + InterruptConfigHandler<C> + InterruptBarHandler<C>,
{
}

pub struct NoInterrupt {}

impl<C: PciInterruptController> InterruptSignaler<C> for NoInterrupt {}
impl<C: PciInterruptController> InterruptConfigHandler<C> for NoInterrupt {}
impl<C: PciInterruptController> InterruptBarHandler<C> for NoInterrupt {}
impl<C: PciInterruptController> IntxHandler<C> for NoInterrupt {}
impl<C: PciInterruptController> MsiHandler<C> for NoInterrupt {}
impl<C: PciInterruptController> MsiXHandler<C> for NoInterrupt {}

impl<C: PciInterruptController> InterruptSignaler<C> for PciIntx {
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        !intx::interrupts_disabled(config)
    }

    fn active_interrupt(&self, config: &PciConfigurationSpace) -> PciInterruptType {
        if !intx::interrupts_disabled(config) {
            PciInterruptType::Intx
        } else {
            PciInterruptType::NoInterrupt
        }
    }

    fn signal(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        interrupt: PciInterrupt,
    ) -> Result<()> {
        handle_intx(config, controller, self, interrupt)
    }
}

impl<C: PciInterruptController> InterruptConfigHandler<C> for PciIntx {
    fn on_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        intx::on_write_config(config, controller, self, offset, size);
        Ok(PciHandlerResult::Handled(None))
    }
}

impl<C: PciInterruptController> IntxHandler<C> for PciIntx {}

impl<T: PciInterruptController> InterruptSignaler<T> for PciMsi<T> {
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        <Self as PciMsiMessageSource>::is_enabled(self, config)
    }

    fn signal(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut T,
        interrupt: PciInterrupt,
    ) -> Result<()> {
        handle_msi(config, controller, self, interrupt)
    }
}

impl<T: PciInterruptController> InterruptConfigHandler<T> for PciMsi<T> {
    fn on_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut T,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        PciConfigurationSpaceIoHandler::on_write_config(self, config, offset, size, controller)
    }
}

impl<T: PciInterruptController> MsiHandler<T> for PciMsi<T> {}

impl<C, M, P> InterruptSignaler<C> for PciMsiX<M, P>
where
    C: PciInterruptController,
    M: MsiXTable,
    P: PbaTable,
{
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        <Self as PciMsiMessageSource>::is_enabled(self, config)
    }

    fn active_interrupt(&self, config: &PciConfigurationSpace) -> PciInterruptType {
        if <Self as PciMsiMessageSource>::is_enabled(self, config) {
            PciInterruptType::MsiX
        } else {
            PciInterruptType::NoInterrupt
        }
    }

    fn signal(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        interrupt: PciInterrupt,
    ) -> Result<()> {
        handle_msi(config, controller, self, interrupt)
    }
}

impl<C, M, P> InterruptConfigHandler<C> for PciMsiX<M, P>
where
    C: PciInterruptController,
    M: MsiXTable,
    P: PbaTable,
{
    fn on_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let mut c: &mut dyn PciInterruptController = controller;
        PciConfigurationSpaceIoHandler::on_write_config(self, config, offset, size, &mut c).map(
            |r| match r {
                PciHandlerResult::Handled(()) => PciHandlerResult::Handled(None),
                PciHandlerResult::Unhandled => PciHandlerResult::Unhandled,
            },
        )
    }
}

impl<C, M, P> InterruptBarHandler<C> for PciMsiX<M, P>
where
    C: PciInterruptController,
    M: MsiXTable,
    P: PbaTable,
{
    fn read_bar(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
    ) -> Result<PciHandlerResult<()>> {
        let mut context = MsiXBarHandlerContext::new(config, controller);
        PciBarRegionSetHandler::handle_read_bar(self, bar, offset, data, &mut context).map(|r| {
            match r {
                PciHandlerResult::Handled(_) => PciHandlerResult::Handled(()),
                PciHandlerResult::Unhandled => PciHandlerResult::Unhandled,
            }
        })
    }

    fn write_bar(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let mut context = MsiXBarHandlerContext::new(config, controller);
        PciBarRegionSetHandler::handle_write_bar(self, bar, offset, data, &mut context)
    }
}

impl<C, M, P> MsiXHandler<C> for PciMsiX<M, P>
where
    C: PciInterruptController,
    M: MsiXTable,
    P: PbaTable,
{
}

/// Composes INTx, MSI, and MSI-X handlers into a single [`PciInterruptHandler`].
/// Use [`NoInterrupt`] for any handler not supported by the device.
///
/// Priority for [`InterruptSignaler::signal`]: MSI-X > MSI > INTx.
pub struct PciInterruptContext<Ix, Ms, Mx> {
    pub intx: Ix,
    pub msi: Ms,
    pub msix: Mx,
}

impl<Ix, Ms, Mx> PciInterruptContext<Ix, Ms, Mx> {
    pub fn new(intx: Ix, msi: Ms, msix: Mx) -> Self {
        Self { intx, msi, msix }
    }
}

impl<C, Ix, Ms, Mx> InterruptSignaler<C> for PciInterruptContext<Ix, Ms, Mx>
where
    C: PciInterruptController,
    Ix: IntxHandler<C>,
    Ms: MsiHandler<C>,
    Mx: MsiXHandler<C>,
{
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        self.msix.is_enabled(config) || self.msi.is_enabled(config) || self.intx.is_enabled(config)
    }

    fn active_interrupt(&self, config: &PciConfigurationSpace) -> PciInterruptType {
        if self.msix.is_enabled(config) {
            return PciInterruptType::MsiX;
        }
        if self.msi.is_enabled(config) {
            return PciInterruptType::Msi;
        }
        if self.intx.is_enabled(config) {
            return PciInterruptType::Intx;
        }
        PciInterruptType::NoInterrupt
    }

    fn signal(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        interrupt: PciInterrupt,
    ) -> Result<()> {
        match self.active_interrupt(config) {
            PciInterruptType::Intx => self.intx.signal(config, controller, interrupt),
            PciInterruptType::Msi => self.msi.signal(config, controller, interrupt),
            PciInterruptType::MsiX => self.msix.signal(config, controller, interrupt),
            PciInterruptType::NoInterrupt => Ok(()),
        }
    }
}

impl<C, Ix, Ms, Mx> InterruptConfigHandler<C> for PciInterruptContext<Ix, Ms, Mx>
where
    C: PciInterruptController,
    Ix: IntxHandler<C>,
    Ms: MsiHandler<C>,
    Mx: MsiXHandler<C>,
{
    fn on_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        // All three handlers must process every config write; their control
        // register regions don't overlap so at most one will produce an update.
        self.intx
            .on_write_config(config, controller, offset, size)?;

        if let PciHandlerResult::Handled(Some(event)) =
            self.msi.on_write_config(config, controller, offset, size)?
        {
            return Ok(PciHandlerResult::Handled(Some(event)));
        }

        if let PciHandlerResult::Handled(Some(event)) = self
            .msix
            .on_write_config(config, controller, offset, size)?
        {
            return Ok(PciHandlerResult::Handled(Some(event)));
        }

        Ok(PciHandlerResult::Handled(None))
    }
}

impl<C, Ix, Ms, Mx> InterruptBarHandler<C> for PciInterruptContext<Ix, Ms, Mx>
where
    C: PciInterruptController,
    Ix: IntxHandler<C>,
    Ms: MsiHandler<C>,
    Mx: MsiXHandler<C>,
{
    fn read_bar(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
    ) -> Result<PciHandlerResult<()>> {
        self.msix.read_bar(config, controller, bar, offset, data)
    }

    fn write_bar(
        &mut self,
        config: &mut PciConfigurationSpace,
        controller: &mut C,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        self.msix.write_bar(config, controller, bar, offset, data)
    }
}

fn handle_msi(
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

fn handle_intx(
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
