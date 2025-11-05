// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::marker::PhantomData;

use greens_pci::bar::PciBarIndex;
use greens_pci::bar_region::{PciBarRegion, PciBarRegionHandler, PciBarRegionInfo};
use greens_pci::capability::{PciCapability, PciCapabilityId};
use greens_pci::Result;

use crate::pci_cap::{VirtioPciCap, VirtioPciCapType};

pub struct VirtioPciIsrCfgCap(VirtioPciCap);

impl PciCapability for VirtioPciIsrCfgCap {
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

impl VirtioPciIsrCfgCap {
    pub fn new(bar: u8, offset: u32, length: u32) -> Self {
        Self(VirtioPciCap::new(
            None,
            VirtioPciCapType::IsrCfg,
            bar,
            0,
            offset,
            length,
        ))
    }
}

const REG_ISR_STATUS: usize = 0;

#[derive(Debug, Clone)]
pub struct VirtioPciIsr<T: VirtioPciIsrState> {
    region: PciBarRegionInfo,
    phantom: PhantomData<T>,
}

impl<T> PciBarRegion for VirtioPciIsr<T>
where
    T: VirtioPciIsrState,
{
    fn info(&self) -> &PciBarRegionInfo {
        &self.region
    }
}

impl<T> PciBarRegionHandler for VirtioPciIsr<T>
where
    T: VirtioPciIsrState,
{
    type R = ();
    type Context<'a> = T;

    fn read_bar(
        &mut self,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        // Reads outside status byte returns 0
        data.fill(0);

        if offset == REG_ISR_STATUS as u64 {
            // We only care about the first byte
            if let Some(byte) = data.get_mut(0) {
                *byte = context.read_and_clear_isr();
            }
        }

        Ok(())
    }

    fn write_bar(
        &mut self,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        let _ = context;
        let _ = offset;
        let _ = data;
        Ok(())
    }
}

impl<T> VirtioPciIsr<T>
where
    T: VirtioPciIsrState,
{
    pub fn new(bar: PciBarIndex, offset: u64, length: u64) -> Self {
        Self {
            region: PciBarRegionInfo::new(bar, offset, length),
            phantom: PhantomData,
        }
    }
}

pub const INTERRUPT_STATUS_USED_BUFFER: u8 = 0b01;
pub const INTERRUPT_STATUS_CONFIGURATION_CHANGE: u8 = 0b10;

pub trait VirtioPciIsrState {
    fn read_and_clear_isr(&mut self) -> u8;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct VirtioInterruptStatus(u8);

impl VirtioInterruptStatus {
    pub fn read_and_clear(&mut self) -> u8 {
        let isr = self.0;
        self.0 = 0;
        isr
    }

    pub fn interrupts_pending(&self) -> bool {
        self.0 != 0
    }

    pub fn set_used_buffer_notification(&mut self) {
        self.0 |= INTERRUPT_STATUS_USED_BUFFER;
    }

    pub fn set_configuration_change_notification(&mut self) {
        self.0 |= INTERRUPT_STATUS_CONFIGURATION_CHANGE;
    }
}

#[cfg(test)]
mod tests {
    use greens_pci::configuration_space::PciConfigurationSpace;

    use crate::pci_cap::tests::{check_cap, check_cap_offs_len, check_cap_ro_fields};
    use crate::pci_cap::VIRTIO_CAP_SIZE;

    use super::*;

    const TEST_ISR_OFF: u64 = 0x12;
    const TEST_ISR_SIZE: usize = 0x1;

    struct TestIsr(u8);

    impl VirtioPciIsrState for TestIsr {
        fn read_and_clear_isr(&mut self) -> u8 {
            let val = self.0;
            self.0 = 0;
            val
        }
    }

    fn new_isr_cfg() -> VirtioPciIsr<TestIsr> {
        VirtioPciIsr::new(
            PciBarIndex::try_from(1).unwrap(),
            TEST_ISR_OFF,
            TEST_ISR_SIZE as u64,
        )
    }

    #[test]
    fn test_isr_cfg_cap() {
        let mut config = PciConfigurationSpace::new();

        let bar = 1;
        let offs = 0x1000;
        let len = 0x2000;

        config
            .add_capability(&VirtioPciIsrCfgCap::new(bar, offs, len))
            .expect("adding cap failed");

        check_cap(&config, VIRTIO_CAP_SIZE, VirtioPciCapType::IsrCfg, bar);
        check_cap_offs_len(&config, offs, len);

        // all fields RO
        check_cap_ro_fields(&mut config, &[0x00u8; VIRTIO_CAP_SIZE])
    }

    #[test]
    fn test_isr_cfg_read_bar_other_bars() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(0);
        let mut data = [0xFFu8; TEST_ISR_SIZE];

        // Ignores reads other BARs
        let bar = PciBarIndex::try_from(0).unwrap();
        let r = cfg
            .handle_read_bar(bar, TEST_ISR_OFF, &mut data, &mut isr)
            .unwrap();

        assert!(!r.handled());
        assert_eq!(data[0], 0xFF);
    }

    #[test]
    fn test_isr_cfg_read_bar_outside_region() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(0);

        let mut data = [0xFFu8; TEST_ISR_SIZE];

        // Ignores reads outside the region
        let bar = PciBarIndex::try_from(1).unwrap();
        let r = cfg.handle_read_bar(bar, 0, &mut data, &mut isr).unwrap();

        assert!(!r.handled());
        assert_eq!(data[0], 0xFF);
    }

    fn check_handle_read(cfg: &mut VirtioPciIsr<TestIsr>, isr: &mut TestIsr, expect: u8) {
        let mut data = [0xFFu8; TEST_ISR_SIZE];
        let bar = PciBarIndex::try_from(1).unwrap();

        assert_eq!(isr.0, expect);

        let r = cfg
            .handle_read_bar(bar, TEST_ISR_OFF, &mut data, isr)
            .unwrap();

        // Handles read
        assert!(r.handled());
        // Data is expected
        assert_eq!(data[0], expect);
        // Interrupts are cleared
        assert_eq!(isr.0, 0);
    }

    #[test]
    fn test_isr_cfg_read_bar_used_notif() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(INTERRUPT_STATUS_USED_BUFFER);

        check_handle_read(&mut cfg, &mut isr, INTERRUPT_STATUS_USED_BUFFER);
    }

    #[test]
    fn test_isr_cfg_read_bar_configuration_change_notif() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(INTERRUPT_STATUS_CONFIGURATION_CHANGE);

        check_handle_read(&mut cfg, &mut isr, INTERRUPT_STATUS_CONFIGURATION_CHANGE);
    }

    #[test]
    fn test_isr_cfg_read_bar_both() {
        let mut cfg = new_isr_cfg();
        let expect = INTERRUPT_STATUS_USED_BUFFER | INTERRUPT_STATUS_CONFIGURATION_CHANGE;
        let mut isr = TestIsr(expect);

        check_handle_read(&mut cfg, &mut isr, expect);
    }

    #[test]
    fn test_isr_cfg_read_bar_no_interrupt() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(0);

        check_handle_read(&mut cfg, &mut isr, 0);
    }

    #[test]
    fn test_isr_cfg_write_bar_other_bars() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(INTERRUPT_STATUS_USED_BUFFER);
        let data = [0xFFu8; TEST_ISR_SIZE];

        // Ignores writes other BARs
        let bar = PciBarIndex::try_from(0).unwrap();
        let r = cfg
            .handle_write_bar(bar, TEST_ISR_OFF, &data, &mut isr)
            .unwrap();

        assert!(!r.handled());
        assert_eq!(isr.0, INTERRUPT_STATUS_USED_BUFFER);
    }

    #[test]
    fn test_isr_cfg_write_bar_outside_region() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(INTERRUPT_STATUS_USED_BUFFER);
        let data = [0xFFu8; TEST_ISR_SIZE];

        // Ignores writes outside region
        let bar = PciBarIndex::try_from(1).unwrap();
        let r = cfg.handle_write_bar(bar, 0, &data, &mut isr).unwrap();

        assert!(!r.handled());
        assert_ne!(isr.0, 0);
    }

    #[test]
    fn test_isr_cfg_write_bar() {
        let mut cfg = new_isr_cfg();
        let mut isr = TestIsr(INTERRUPT_STATUS_USED_BUFFER);
        let data = [0xFFu8; TEST_ISR_SIZE];

        // Ignores writes, but reports as handled
        let bar = PciBarIndex::try_from(1).unwrap();
        let r = cfg
            .handle_write_bar(bar, TEST_ISR_OFF, &data, &mut isr)
            .unwrap();

        assert!(r.handled());
        assert_ne!(isr.0, 0);
    }
}
