// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::Result;
use greens_pci::bar::PciBarIndex;
use greens_pci::bar_region::{PciBarRegion, PciBarRegionHandler, PciBarRegionInfo};
use greens_pci::capability::{PciCapability, PciCapabilityId};
use virtio_queue::Queue;

use crate::pci::VirtioPciDevice;
use crate::pci_cap::{VirtioPciCap, VirtioPciCapType};

pub struct VirtioPciDeviceCfgCap(VirtioPciCap);

impl VirtioPciDeviceCfgCap {
    pub fn new(bar: u8, offset: u32, length: u32) -> Self {
        Self(VirtioPciCap::new(
            None,
            VirtioPciCapType::DeviceCfg,
            bar,
            0,
            offset,
            length,
        ))
    }
}

impl PciCapability for VirtioPciDeviceCfgCap {
    fn id(&self) -> PciCapabilityId {
        self.0.id()
    }

    fn size(&self) -> usize {
        self.0.size()
    }

    fn registers(&self, registers: &mut [u8]) {
        self.0.registers(registers)
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        self.0.writable_bits(writable_bits)
    }
}

#[derive(Debug, Clone)]
pub struct VirtioPciDeviceCfg {
    info: PciBarRegionInfo,
}

impl VirtioPciDeviceCfg {
    pub fn new(bar: PciBarIndex, offset: u64, length: u64) -> Self {
        Self {
            info: PciBarRegionInfo::new(bar, offset, length),
        }
    }
}

impl PciBarRegion for VirtioPciDeviceCfg {
    fn info(&self) -> &PciBarRegionInfo {
        &self.info
    }
}

impl PciBarRegionHandler for VirtioPciDeviceCfg {
    type Context<'a> = &'a mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue>;
    type R = ();

    fn read_bar(
        &mut self,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        context.read_config(offset as usize, data);

        Ok(())
    }

    fn write_bar(
        &mut self,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        context.write_config(offset as usize, data);

        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use greens_pci::configuration_space::PciConfigurationSpace;

    use super::*;
    use crate::pci_cap::VIRTIO_CAP_SIZE;
    use crate::pci_cap::tests::{check_cap, check_cap_offs_len, check_cap_ro_fields};
    use crate::pci_common_cfg::tests::TestDevice;

    #[test]
    fn test_device_cfg_cap() {
        let mut config = PciConfigurationSpace::new();

        let bar = 1;
        let offs = 0x1000;
        let len = 0x2000;

        config
            .add_capability(&VirtioPciDeviceCfgCap::new(bar, offs, len))
            .expect("adding cap failed");

        check_cap(&config, VIRTIO_CAP_SIZE, VirtioPciCapType::DeviceCfg, bar);
        check_cap_offs_len(&config, offs, len);

        // all fields RO
        check_cap_ro_fields(&mut config, &[0x00u8; VIRTIO_CAP_SIZE])
    }

    fn read(cfgdev: &mut (VirtioPciDeviceCfg, TestDevice), offset: u64, data: &mut [u8]) {
        let (cfg, dev) = cfgdev;
        let mut dev: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue> = dev;
        cfg.read_bar(offset, data, &mut dev).expect("read")
    }

    fn write(cfgdev: &mut (VirtioPciDeviceCfg, TestDevice), offset: u64, data: &[u8]) {
        let (cfg, dev) = cfgdev;
        let mut dev: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue> = dev;
        cfg.write_bar(offset, &data, &mut dev).expect("write")
    }

    fn test_dev() -> (VirtioPciDeviceCfg, TestDevice) {
        let dev = TestDevice::new();
        let size = dev.cfg.config_space.len() as u64;

        (
            VirtioPciDeviceCfg::new(PciBarIndex::default(), 0, size),
            dev,
        )
    }

    #[test]
    fn test_device_cfg_read() {
        let mut cfgdev = test_dev();

        let mut d = [0u8; 4];
        let mut offs: usize = 0;
        read(&mut cfgdev, offs as u64, &mut d);
        assert_eq!(d, cfgdev.1.cfg.config_space[offs..offs + d.len()]);

        offs = 4;
        read(&mut cfgdev, offs as u64, &mut d);
        assert_eq!(d, cfgdev.1.cfg.config_space[offs..offs + d.len()]);

        let mut d = [0u8; 8];
        offs = 0;
        read(&mut cfgdev, offs as u64, &mut d);
        assert_eq!(d, cfgdev.1.cfg.config_space[offs..offs + d.len()]);

        // reading out of bounds; read data should remain untouched
        let mut d = [0xFF; 2];
        offs = cfgdev.1.cfg.config_space.len();
        read(&mut cfgdev, offs as u64, &mut d);
        assert!(d.iter().all(|x| *x == 0xFF))
    }

    #[test]
    fn test_device_cfg_write() {
        let mut cfgdev = test_dev();
        let d = [0u8; 4];
        let mut offs: usize = 0;
        write(&mut cfgdev, offs as u64, &d);
        assert_eq!(d, cfgdev.1.cfg.config_space[offs..offs + d.len()]);

        let mut cfgdev = test_dev();
        offs = 4;
        write(&mut cfgdev, offs as u64, &d);
        assert_eq!(d, cfgdev.1.cfg.config_space[offs..offs + d.len()]);

        let mut cfgdev = test_dev();
        let d = [0u8; 8];
        offs = 0;
        write(&mut cfgdev, offs as u64, &d);
        assert_eq!(d, cfgdev.1.cfg.config_space[offs..offs + d.len()]);

        // writing out of bounds; should leave data untouched
        let mut cfgdev = test_dev();
        let d = [0xFF; 2];
        let expect = cfgdev.1.cfg.config_space.clone();
        offs = cfgdev.1.cfg.config_space.len();
        write(&mut cfgdev, offs as u64, &d);
        assert_eq!(cfgdev.1.cfg.config_space, expect);
    }
}
