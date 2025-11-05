// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::bar::PciBarIndex;
use greens_pci::bar_region::{PciBarRegion, PciBarRegionHandler, PciBarRegionInfo};
use greens_pci::capability::{PciCapOffset, PciCapability, PciCapabilityId};
use greens_pci::config_handler::PciConfigurationSpaceIoHandler;
use greens_pci::function::PciHandlerResult;
use greens_pci::registers::PCI_BAR0;
use greens_pci::utils::register_block::set_dword;
use greens_pci::{Error, Result};
use virtio_queue::Queue;

use crate::pci::VirtioPciDevice;
use crate::pci_cap::{virtio_cap_len, VirtioPciCap, VirtioPciCapType, VIRTIO_CAP_SIZE};

pub(crate) const REG_NOTIFY_OFF_MULTIPLIER: usize = 14;
pub(crate) const VIRTIO_NOTIFY_CAP_SIZE: usize = VIRTIO_CAP_SIZE + 4;

pub struct VirtioPciNotifyCfgCap {
    cap: VirtioPciCap,
    notify_off_multiplier: u32,
}

impl VirtioPciNotifyCfgCap {
    pub fn new(bar: u8, offset: u32, length: u32, notify_off_multiplier: Option<u32>) -> Self {
        let cap = VirtioPciCap::new(
            Some(virtio_cap_len(VIRTIO_NOTIFY_CAP_SIZE)),
            VirtioPciCapType::NofifyCfg,
            bar,
            0,
            offset,
            length,
        );
        let notify_off_multiplier = notify_off_multiplier.unwrap_or(0);
        Self {
            cap,
            notify_off_multiplier,
        }
    }
}

impl PciCapability for VirtioPciNotifyCfgCap {
    fn id(&self) -> PciCapabilityId {
        self.cap.id()
    }

    fn size(&self) -> usize {
        VIRTIO_NOTIFY_CAP_SIZE
    }

    fn registers(&self, registers: &mut [u8]) {
        self.cap.registers(&mut registers[0..VIRTIO_CAP_SIZE]);
        set_dword(
            registers,
            REG_NOTIFY_OFF_MULTIPLIER,
            self.notify_off_multiplier,
        )
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        // all R/O
        writable_bits.fill(0);
    }
}

#[derive(Debug, Clone)]
pub struct VirtioPciNotifyCfg {
    cap_offset: PciCapOffset,
    info: PciBarRegionInfo,
}

impl PciBarRegionHandler for VirtioPciNotifyCfg {
    type Context<'a> = &'a mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue>;
    type R = ();

    fn read_bar(
        &mut self,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        let _ = offset;
        let _ = data;
        let _ = context;

        Ok(())
    }

    fn write_bar(
        &mut self,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        let _ = offset;

        // Although the driver writes either 16- or 32-bits, be prepared for wider access.
        let mut value = [0u8; 8];
        value[..data.len()].copy_from_slice(data);
        let value = u64::from_ne_bytes(value) as u32;
        context.queue_notify(value);

        Ok(())
    }
}

impl PciBarRegion for VirtioPciNotifyCfg {
    fn info(&self) -> &PciBarRegionInfo {
        &self.info
    }
}

impl PciConfigurationSpaceIoHandler for VirtioPciNotifyCfg {
    type Context<'a> = &'a mut dyn VirtioPciDevice<E = Error, Q = Queue>;
    type R = ();

    fn postprocess_write_config(
        &mut self,
        config: &mut greens_pci::configuration_space::PciConfigurationSpace,
        offset: usize,
        size: usize,
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        if self.targets_bar_cfg(offset, size) {
            let Some(bar_gpa) = config.get_bar(self.info.bar).and_then(|bar| bar.address()) else {
                return Ok(PciHandlerResult::Unhandled);
            };

            context.set_notification_info(VirtioPciNotifyCfgInfo {
                bar_gpa: bar_gpa + self.info.offset,
                bar_len: self.info.length,
                queue_notify_off: config.read_dword(self.cap_offset + REG_NOTIFY_OFF_MULTIPLIER),
            });
        }
        Ok(PciHandlerResult::Handled(()))
    }
}

impl VirtioPciNotifyCfg {
    pub fn new(cap_offset: PciCapOffset, bar: PciBarIndex, offset: u64, length: u64) -> Self {
        Self {
            cap_offset,
            info: PciBarRegionInfo::new(bar, offset, length),
        }
    }

    pub fn targets_bar_cfg(&self, offset: usize, size: usize) -> bool {
        let bar_offset = PCI_BAR0 + (self.info.bar.into_inner() * 4);

        bar_offset >= offset && bar_offset < bar_offset + size
    }
}

/// A structure containing the the configuration information for a Virtio PCI notification
/// region.
///
/// Using the information the VirtIO device can determine the queue notification addresses.
pub struct VirtioPciNotifyCfgInfo {
    /// The Guest Physical Address (GPA) pointing to the beginning of the notification BAR region.
    pub bar_gpa: u64,
    /// The length of the BAR region in bytes.
    pub bar_len: u64,
    /// The offset multiplier for calculating the queue notification addresses.
    ///
    /// The queue notification addresses are derived from:
    /// `notify_gpa = base_gpa + queue_notify_off * notify_off_multiplier`, where
    /// `notify_off_multiplier` is typically the queue number.
    pub queue_notify_off: u32,
}

impl VirtioPciNotifyCfgInfo {
    /// Calculates GPA for `notify_off_multiplier`, which typically implies queue number.
    pub fn notification_gpa(&self, notify_off_multiplier: u16) -> Option<u64> {
        let notify_gpa =
            self.bar_gpa + u64::from(self.queue_notify_off) * u64::from(notify_off_multiplier);
        if notify_gpa < self.bar_gpa + self.bar_len {
            Some(notify_gpa)
        } else {
            None
        }
    }
}

#[cfg(test)]
pub mod tests {
    use greens_pci::configuration_space::PciConfigurationSpace;

    use crate::pci_cap::tests::{check_cap, check_cap_offs_len, check_cap_ro_fields};

    use super::*;

    fn check_notify_multiplier(config: &mut PciConfigurationSpace, val: u32) {
        let (_, cap_offset) = config.capability_iter().last().unwrap();
        assert_eq!(
            config.read_dword(cap_offset + REG_NOTIFY_OFF_MULTIPLIER),
            val
        );
    }

    #[test]
    fn test_notify_cap() {
        let mut config = PciConfigurationSpace::new();

        let bar = 1;
        let offs = 0x1000;
        let len = 0x2000;

        config
            .add_capability(&VirtioPciNotifyCfgCap::new(bar, offs, len, None))
            .expect("adding cap failed");

        check_cap(
            &config,
            VIRTIO_NOTIFY_CAP_SIZE,
            VirtioPciCapType::NofifyCfg,
            bar,
        );
        check_cap_offs_len(&config, offs, len);
        check_notify_multiplier(&mut config, 0);

        // all fields RO
        check_cap_ro_fields(&mut config, &[0x00u8; VIRTIO_NOTIFY_CAP_SIZE]);

        // Add another cap and check that multiplier is set
        config
            .add_capability(&VirtioPciNotifyCfgCap::new(bar, offs, len, Some(10)))
            .expect("adding cap failed");
        check_notify_multiplier(&mut config, 10);
    }
}
