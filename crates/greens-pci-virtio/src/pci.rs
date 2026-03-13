// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::bar::{PciBar, PciBarIndex, PciBarPrefetchable::NotPrefetchable, PciBarType};
use greens_pci::bar_region::{PciBarRegionHandler, PciBarRegionInfo, PciBarRegionSetHandler};
use greens_pci::config_handler::PciConfigurationSpaceIoHandler;
use greens_pci::configuration_space::PciConfigurationSpace;
use greens_pci::function::{
    PciConfigurationUpdate, PciFunctionConfig, PciFunctionConfigAccessor,
    PciFunctionWithInterrupts, PciHandlerResult, PciInterruptConfigHandler,
};
use greens_pci::interrupt::PciInterruptType::{Intx, MsiX, NoInterrupt};
use greens_pci::interrupt::{PciInterruptSignaler, handle_intx, handle_msi};
use greens_pci::intx;
use greens_pci::intx::{PciInterruptLineConfig, PciIntx, PciIntxConfig, PciIntxPin};
use greens_pci::msi::PciMsiMessageSource;
use greens_pci::msix::{MsiXBarHandlerContext, MsiXEntry, PbaEntry, PciMsiX, PciMsiXConfig};
use greens_pci::{PciInterruptController, PciMsiMessage, Result};
use virtio_device::{VirtioDevice, WithDriverSelect};
use virtio_queue::Queue;

use crate::pci_cfg_cap::{VirtioPciCfg, VirtioPciCfgCap, VirtioPciCfgHandler};
use crate::pci_common_cfg::{VirtioPciCommonCfg, VirtioPciCommonCfgCap};
use crate::pci_device_cfg::{VirtioPciDeviceCfg, VirtioPciDeviceCfgCap};
use crate::pci_isr_cfg::{VirtioPciIsr, VirtioPciIsrCfgCap, VirtioPciIsrState};
use crate::pci_notify_cfg::{VirtioPciNotifyCfg, VirtioPciNotifyCfgCap, VirtioPciNotifyCfgInfo};

pub const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
pub const VIRTIO_PCI_DEVICE_ID_BASE: u16 = 0x1040;
pub const VIRTIO_PCI_REVISION_ID: u8 = 1;

type VirtioMsiXTable = [MsiXEntry; 32];
type VirtioPbaTable = [PbaEntry; 1];

pub trait VirtioPciDevice: VirtioDevice + WithDriverSelect + VirtioPciIsrState {
    fn set_config_msix_vector(&mut self, vector: u16);
    fn config_msix_vector(&self) -> u16;

    /// Sets the queue notification configuration information for the device.
    ///
    /// The method is called when the guest maps the notification BAR.
    fn set_notification_info(&mut self, notify_cfg_info: VirtioPciNotifyCfgInfo);
    fn queue_notify(&mut self, data: u32);

    fn set_msi_message(&mut self, vector: u16, msg: PciMsiMessage);

    fn set_queue_msix_vector(&mut self, vector: u16);
    fn queue_msix_vector(&self) -> Option<u16>;
}

pub struct VirtioPciFunction<
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
> {
    config: PciConfigurationSpace,
    common_cfg: VirtioPciCommonCfg,
    device_cfg: VirtioPciDeviceCfg,
    isr_cfg: VirtioPciIsr<D>,
    pci_cfg: VirtioPciCfg,
    notify_cfg: VirtioPciNotifyCfg,
    interrupt_controller: C,
    msix: PciMsiX<VirtioMsiXTable, VirtioPbaTable>,
    intx: PciIntx,
    device: D,
}

impl<C, D> VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    pub fn new(
        pci_class_base: u8,
        pci_class_sub: u8,
        interrupt_line: PciInterruptLineConfig,
        interrupt_controller: C,
        device: D,
    ) -> Result<Self> {
        let pci_device_id = VIRTIO_PCI_DEVICE_ID_BASE + device.device_type() as u16;

        // TODO: Extend builder to cover BARs and capabilities with closures?
        let mut config = PciFunctionConfig::new()
            .with_id(VIRTIO_PCI_VENDOR_ID, pci_device_id, VIRTIO_PCI_REVISION_ID)
            .with_class(pci_class_base, pci_class_sub, 0x00)
            .with_subsystem_id(VIRTIO_PCI_VENDOR_ID, pci_device_id)
            .build();

        let bar_index = PciBarIndex::try_from(0)?;
        let config_bar = PciBar::new(
            None,
            0x8000,
            bar_index,
            PciBarType::Memory32Bit(NotPrefetchable),
        );
        config.add_bar(config_bar)?;

        let common_cfg_size = VirtioPciCommonCfg::size() as u32;

        config.add_capability(&VirtioPciCommonCfgCap::new(0, 0x0000, common_cfg_size))?;
        config.add_capability(&VirtioPciIsrCfgCap::new(0, 0x1000, 1))?;
        config.add_capability(&VirtioPciDeviceCfgCap::new(0, 0x2000, 0x1000))?;
        let notify_cap_offs =
            config.add_capability(&VirtioPciNotifyCfgCap::new(0, 0x3000, 0x1000, Some(4)))?;

        let common_cfg = VirtioPciCommonCfg::new(bar_index, 0x0000, common_cfg_size as u64);
        let device_cfg = VirtioPciDeviceCfg::new(bar_index, 0x2000, 0x1000);
        let isr_cfg = VirtioPciIsr::new(bar_index, 0x1000, 1);
        let pci_cfg = VirtioPciCfg::new(config.add_capability(&VirtioPciCfgCap::new())?);
        let notify_cfg = VirtioPciNotifyCfg::new(notify_cap_offs, bar_index, 0x3000, 0x1000);

        // Add MSI-X cap
        let msix = PciMsiXConfig::new(
            PciBarRegionInfo::new(bar_index, 0x4000, 0x1000),
            PciBarRegionInfo::new(bar_index, 0x5000, 0x1000),
        )
        .build(
            &mut config,
            [MsiXEntry::default(); 32],
            [PbaEntry::default(); 1],
        )?;

        let intx = PciIntxConfig::new(PciIntxPin::IntA, interrupt_line).build(&mut config);

        Ok(Self {
            config,
            common_cfg,
            device_cfg,
            pci_cfg,
            isr_cfg,
            notify_cfg,
            device,
            msix,
            intx,
            interrupt_controller,
        })
    }
}

impl<C, D> PciInterruptConfigHandler for VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    fn on_interrupt_config_update(&mut self, event: PciConfigurationUpdate) {
        if let PciConfigurationUpdate::MsiXMessage(vector, msg) = event {
            self.device.set_msi_message(vector as u16, msg)
        }
    }

    fn device_read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()> {
        self.pci_cfg_cap().prepare_read(self, offset, data.len());
        self.config.read_checked(offset, data)
    }

    fn device_write_config(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        self.config.write_checked(offset, data)?;
        let mut device: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue> =
            &mut self.device;
        self.notify_cfg.postprocess_write_config(
            &mut self.config,
            offset,
            data.len(),
            &mut device,
        )?;
        self.pci_cfg_cap().process_write(self, offset, data.len());
        Ok(())
    }

    fn device_read_bar(&mut self, bar: PciBarIndex, offset: u64, data: &mut [u8]) -> Result<()> {
        let mut device: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue> =
            &mut self.device;

        if self
            .common_cfg
            .handle_read_bar(bar, offset, data, &mut device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .device_cfg
            .handle_read_bar(bar, offset, data, &mut device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .notify_cfg
            .handle_read_bar(bar, offset, data, &mut device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .isr_cfg
            .handle_read_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            return Ok(());
        }

        Ok(())
    }

    fn device_write_bar(&mut self, bar: PciBarIndex, offset: u64, data: &[u8]) -> Result<()> {
        let mut device: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue> =
            &mut self.device;

        if self
            .common_cfg
            .handle_write_bar(bar, offset, data, &mut device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .device_cfg
            .handle_write_bar(bar, offset, data, &mut device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .notify_cfg
            .handle_write_bar(bar, offset, data, &mut device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .isr_cfg
            .handle_write_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            return Ok(());
        }

        Ok(())
    }
}

impl<C, D> PciFunctionConfigAccessor for VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    fn config(&self) -> &PciConfigurationSpace {
        &self.config
    }

    fn config_mut(&mut self) -> &mut PciConfigurationSpace {
        &mut self.config
    }
}

impl<C, D> PciInterruptSignaler for VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    fn signal_interrupt(&mut self, interrupt: greens_pci::interrupt::PciInterrupt) -> Result<()> {
        match self.active_interrupt() {
            Intx => handle_intx(
                &mut self.config,
                &mut self.interrupt_controller,
                &self.intx,
                interrupt,
            ),
            MsiX => handle_msi(
                &mut self.config,
                &mut self.interrupt_controller,
                &mut self.msix,
                interrupt,
            ),
            _ => Ok(()),
        }
    }

    fn active_interrupt(&self) -> greens_pci::interrupt::PciInterruptType {
        if self.msix.is_enabled(&self.config) {
            return MsiX;
        }

        if !intx::interrupts_disabled(&self.config) {
            return Intx;
        }

        NoInterrupt
    }

    fn postprocess_write_config(
        &mut self,
        offset: usize,
        size: usize,
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        // TODO: process write to handle interrupt source changes:
        //      - INTx is asserted, MSI(-X) enabled -> deassert INTx (call intx.disable())
        //      - MSI enabled and MSI-X gets enabled -> must not evaluate pending interrupts.
        // FIXME: intx and msi handling should support "force disable", because if higher
        // precedence interrupt is enabled, they should not signal interrupts.
        // They could encode instruction to return when interrupts needs to be evaluated?
        intx::postprocess_write_config(
            &mut self.config,
            &mut self.interrupt_controller,
            &mut self.intx,
            offset,
            size,
        );

        let mut controller: &mut dyn PciInterruptController = &mut self.interrupt_controller;
        self.msix
            .postprocess_write_config(&mut self.config, offset, size, &mut controller)?;

        Ok(PciHandlerResult::Unhandled)
    }

    fn handle_read_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let context =
            &mut MsiXBarHandlerContext::new(&mut self.config, &mut self.interrupt_controller);

        self.msix.handle_read_bar(bar, offset, data, context)
    }

    fn handle_write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<PciHandlerResult<Option<PciConfigurationUpdate>>> {
        let context =
            &mut MsiXBarHandlerContext::new(&mut self.config, &mut self.interrupt_controller);

        self.msix.handle_write_bar(bar, offset, data, context)
    }
}

impl<C, D> PciFunctionWithInterrupts for VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
}

impl<C, D> VirtioPciCfgHandler for VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    fn pci_cfg_cap(&self) -> VirtioPciCfg {
        self.pci_cfg
    }
}
