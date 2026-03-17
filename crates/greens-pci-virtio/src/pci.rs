// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::bar::{PciBar, PciBarIndex, PciBarPrefetchable::NotPrefetchable, PciBarType};
use greens_pci::bar_region::{PciBarRegionHandler, PciBarRegionInfo};
use greens_pci::config_handler::PciConfigurationSpaceIoHandler;
use greens_pci::configuration_space::PciConfigurationSpace;
use greens_pci::function::{
    PciConfigurationUpdate, PciDeviceHandler, PciFunctionBuilder, PciFunctionConfigAccessor,
    PciFunctionWithInterrupts, PciInterruptAccess,
};
use greens_pci::interrupt::{NoInterrupt, PciInterrupt, PciInterruptContext, PciInterruptType};
use greens_pci::intx::{
    PciInterruptLineConfig, PciInterruptLineState, PciIntx, PciIntxConfig, PciIntxPin,
};
use greens_pci::msix::{MsiXEntry, PbaEntry, PciMsiX, PciMsiXConfig};
use greens_pci::{PciInterruptController, PciMsiMessage, Result};
use virtio_device::{VirtioDevice, WithDriverSelect};
use virtio_queue::Queue;

use crate::pci_cfg_cap::{VirtioPciCfg, VirtioPciCfgCap, VirtioPciCfgHandler};
use crate::pci_common_cfg::{VirtioPciCommonCfg, VirtioPciCommonCfgCap};
use crate::pci_device_cfg::{VirtioPciDeviceCfg, VirtioPciDeviceCfgCap};
use crate::pci_isr_cfg::{VirtioPciIsr, VirtioPciIsrCfgCap, VirtioPciIsrState};
use crate::pci_notify_cfg::{VirtioPciNotify, VirtioPciNotifyCfg, VirtioPciNotifyCfgCap};

pub const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
pub const VIRTIO_PCI_DEVICE_ID_BASE: u16 = 0x1040;
pub const VIRTIO_PCI_REVISION_ID: u8 = 1;

type VirtioMsiXTable = [MsiXEntry; 32];
type VirtioPbaTable = [PbaEntry; 1];
type VirtioInterruptContext =
    PciInterruptContext<PciIntx, NoInterrupt, PciMsiX<VirtioMsiXTable, VirtioPbaTable>>;

pub trait VirtioPciDevice:
    VirtioDevice + WithDriverSelect + VirtioPciIsrState + VirtioPciNotify
{
    fn set_config_msix_vector(&mut self, vector: u16);
    fn config_msix_vector(&self) -> u16;

    fn set_msi_message(&mut self, vector: u16, msg: PciMsiMessage);

    fn set_queue_msix_vector(&mut self, vector: u16);
    fn queue_msix_vector(&self) -> Option<u16>;
}

struct VirtioDeviceHandler<D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>> {
    config: PciConfigurationSpace,
    common_cfg: VirtioPciCommonCfg<D>,
    device_cfg: VirtioPciDeviceCfg<D>,
    isr_cfg: VirtioPciIsr<D>,
    notify_cfg: VirtioPciNotifyCfg<D>,
    device: D,
}

impl<D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>> VirtioDeviceHandler<D> {
    fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }
}

impl<D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>> PciFunctionConfigAccessor
    for VirtioDeviceHandler<D>
{
    fn config(&self) -> &PciConfigurationSpace {
        &self.config
    }

    fn config_mut(&mut self) -> &mut PciConfigurationSpace {
        &mut self.config
    }
}

impl<D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>> PciDeviceHandler
    for VirtioDeviceHandler<D>
{
    fn postprocess_write_config(&mut self, offset: usize, size: usize) -> Result<()> {
        self.notify_cfg.postprocess_write_config(
            &mut self.config,
            offset,
            size,
            &mut self.device,
        )?;
        Ok(())
    }

    fn read_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
        irq: &mut PciInterruptAccess<'_>,
    ) -> Result<()> {
        if self
            .common_cfg
            .handle_read_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .device_cfg
            .handle_read_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .notify_cfg
            .handle_read_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .isr_cfg
            .handle_read_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            // Virtio spec: reading the ISR register de-asserts INTx.
            if irq.active() == PciInterruptType::Intx {
                irq.signal(
                    &mut self.config,
                    PciInterrupt::Intx(PciInterruptLineState::Low),
                )?;
            }
            return Ok(());
        }

        Ok(())
    }

    fn write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
        _irq: &mut PciInterruptAccess<'_>,
    ) -> Result<()> {
        if self
            .common_cfg
            .handle_write_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .device_cfg
            .handle_write_bar(bar, offset, data, &mut self.device)?
            .handled()
        {
            return Ok(());
        }

        if self
            .notify_cfg
            .handle_write_bar(bar, offset, data, &mut self.device)?
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

    fn on_interrupt_config_update(&mut self, event: PciConfigurationUpdate) {
        if let PciConfigurationUpdate::MsiXMessage(vector, msg) = event {
            self.device.set_msi_message(vector as u16, msg)
        }
    }
}

pub struct VirtioPciFunction<
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
> {
    inner: PciFunctionWithInterrupts<C, VirtioDeviceHandler<D>, VirtioInterruptContext>,
    pci_cfg: VirtioPciCfg,
}

impl<C, D> VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    /// Access the inner virtio device mutably (e.g. to check pending_notify from the run loop).
    pub fn device_mut(&mut self) -> &mut D {
        self.inner.device_mut().device_mut()
    }

    pub fn signal_interrupt(&mut self, interrupt: PciInterrupt) -> Result<()> {
        self.inner.signal_interrupt(interrupt)
    }

    pub fn new(
        pci_class_base: u8,
        pci_class_sub: u8,
        interrupt_line: PciInterruptLineConfig,
        interrupt_controller: C,
        device: D,
    ) -> Result<Self> {
        let pci_device_id = VIRTIO_PCI_DEVICE_ID_BASE + device.device_type() as u16;

        // TODO: Extend builder to cover BARs and capabilities with closures?
        let mut config = PciFunctionBuilder::new()
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

        let common_cfg_size = VirtioPciCommonCfg::<D>::size() as u32;

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
        let interrupt = PciInterruptContext::new(intx, NoInterrupt {}, msix);

        let device_handler = VirtioDeviceHandler {
            config,
            common_cfg,
            device_cfg,
            isr_cfg,
            notify_cfg,
            device,
        };

        let inner = PciFunctionWithInterrupts::new(interrupt_controller, device_handler, interrupt);

        Ok(Self { inner, pci_cfg })
    }
}

impl<C, D> PciFunctionConfigAccessor for VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    fn config(&self) -> &PciConfigurationSpace {
        self.inner.device().config()
    }

    fn config_mut(&mut self) -> &mut PciConfigurationSpace {
        self.inner.device_mut().config_mut()
    }
}

impl<C, D> greens_pci::function::PciFunction for VirtioPciFunction<C, D>
where
    C: PciInterruptController,
    D: VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
{
    fn read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()> {
        self.pci_cfg_cap().prepare_read(self, offset, data.len());
        self.inner.read_config(offset, data)
    }

    fn write_config(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        self.inner.write_config(offset, data)?;
        self.pci_cfg_cap().process_write(self, offset, data.len());
        Ok(())
    }

    fn get_bar(&self, index: PciBarIndex) -> Option<PciBar> {
        self.inner.get_bar(index)
    }

    fn read_bar(&mut self, bar: PciBarIndex, offset: u64, data: &mut [u8]) -> Result<()> {
        self.inner.read_bar(bar, offset, data)
    }

    fn write_bar(&mut self, bar: PciBarIndex, offset: u64, data: &[u8]) -> Result<()> {
        self.inner.write_bar(bar, offset, data)
    }
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
