// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::capability::{PciCapability, PciCapabilityId};
use greens_pci::utils::set_dword;

// Virtio base cap offsets
// Generic PCI field: capability length.
pub(crate) const REG_CAP_LEN: usize = 0;
// Identifies the structure.
pub(crate) const REG_CAP_CFG: usize = 1;
// Where to find it.
pub(crate) const REG_BAR: usize = 2;
// Multiple capabilities of the same type.
pub(crate) const REG_ID: usize = 3;
// Offset within bar.
pub(crate) const REG_OFFSET: usize = 6;
// Length of the structure, in bytes.
pub(crate) const REG_LENGTH: usize = 10;

// Virtio 64bit cap offsets
// Offset [64:32]
pub(crate) const REG_OFFSET_HI: usize = 14;
// Length [64:32]
pub(crate) const REG_LENGTH_HI: usize = 18;

// Capability sizes
pub(crate) const VIRTIO_CAP_SIZE: usize = REG_OFFSET_HI;
pub(crate) const VIRTIO_CAP64_SIZE: usize = REG_LENGTH_HI + 4;

#[repr(u8)]
#[derive(Debug, Copy, Clone)]
pub enum VirtioPciCapType {
    CommonCfg = 1,
    NofifyCfg = 2,
    IsrCfg = 3,
    DeviceCfg = 4,
    PciCfg = 5,
    SharedMemoryCfg = 8,
    VendorCfg = 9,
}

pub struct VirtioPciCap {
    cap_len: u8,
    cfg_type: VirtioPciCapType,
    bar: u8,
    id: u8,
    offset: u32,
    length: u32,
}

impl VirtioPciCap {
    pub fn new(
        cap_len: Option<u8>,
        cfg_type: VirtioPciCapType,
        bar: u8,
        id: u8,
        offset: u32,
        length: u32,
    ) -> Self {
        let cap_len = cap_len.unwrap_or(virtio_cap_len(VIRTIO_CAP_SIZE));
        Self {
            cap_len,
            cfg_type,
            bar,
            id,
            offset,
            length,
        }
    }
}

impl PciCapability for VirtioPciCap {
    fn id(&self) -> PciCapabilityId {
        PciCapabilityId::VendorSpecific
    }

    fn size(&self) -> usize {
        VIRTIO_CAP_SIZE
    }

    fn registers(&self, registers: &mut [u8]) {
        registers[REG_CAP_LEN] = self.cap_len;
        registers[REG_CAP_CFG] = self.cfg_type as u8;
        registers[REG_BAR] = self.bar;
        registers[REG_ID] = self.id;
        set_dword(registers, REG_OFFSET, self.offset).unwrap();
        set_dword(registers, REG_LENGTH, self.length).unwrap();
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        // all R/O
        writable_bits.fill(0);
    }
}

pub struct VirtioPciCap64 {
    cap: VirtioPciCap,
    offset_hi: u32,
    length_hi: u32,
}

impl VirtioPciCap64 {
    pub fn new(cfg_type: VirtioPciCapType, bar: u8, id: u8, offset: u64, length: u64) -> Self {
        Self {
            cap: VirtioPciCap::new(
                Some(virtio_cap_len(VIRTIO_CAP64_SIZE)),
                cfg_type,
                bar,
                id,
                offset as u32,
                length as u32,
            ),
            offset_hi: (offset >> 32) as u32,
            length_hi: (length >> 32) as u32,
        }
    }
}

impl PciCapability for VirtioPciCap64 {
    fn id(&self) -> PciCapabilityId {
        self.cap.id()
    }

    fn size(&self) -> usize {
        VIRTIO_CAP64_SIZE
    }

    fn registers(&self, registers: &mut [u8]) {
        self.cap.registers(&mut registers[0..VIRTIO_CAP_SIZE]);
        set_dword(registers, REG_OFFSET_HI, self.offset_hi).unwrap();
        set_dword(registers, REG_LENGTH_HI, self.length_hi).unwrap();
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        // all R/O
        writable_bits.fill(0);
    }
}

pub struct VirtioPciSharedMemoryCap(VirtioPciCap64);

impl VirtioPciSharedMemoryCap {
    pub fn new(bar: u8, shm_id: u8, offset: u64, length: u64) -> Self {
        Self(VirtioPciCap64::new(
            VirtioPciCapType::SharedMemoryCfg,
            bar,
            shm_id,
            offset,
            length,
        ))
    }
}

impl PciCapability for VirtioPciSharedMemoryCap {
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

pub(crate) fn virtio_cap_len(cap_size: usize) -> u8 {
    // header (cap_vndr + cap_next) + actual cap
    2 + cap_size as u8
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use greens_pci::configuration_space::PciConfigurationSpace;

    pub fn check_cap(
        config: &PciConfigurationSpace,
        cap_size: usize,
        cap_cfg: VirtioPciCapType,
        bar: u8,
    ) {
        let (header, offset) = config.capability_iter().last().unwrap();

        // cap_id is correct
        assert_eq!(header.cap_id(), PciCapabilityId::VendorSpecific.into());

        // cap_len is correct: header + cap size
        assert_eq!(config.read_byte(offset), 2 + cap_size as u8);

        // cap_cfg is correct
        assert_eq!(config.read_byte(offset + REG_CAP_CFG), cap_cfg as u8);

        // bar is as set
        assert_eq!(config.read_byte(offset + REG_BAR), bar);
    }

    pub fn check_cap_offs_len(config: &PciConfigurationSpace, offset: u32, length: u32) {
        let (_, cap_offset) = config.capability_iter().last().unwrap();

        // offset is as set
        assert_eq!(config.read_dword(cap_offset + REG_OFFSET), offset);

        // length is as set
        assert_eq!(config.read_dword(cap_offset + REG_LENGTH), length);
    }

    pub fn check_cap64_offs_len(config: &PciConfigurationSpace, offset: u64, length: u64) {
        check_cap_offs_len(config, offset as u32, length as u32);

        let (_, cap_offset) = config.capability_iter().last().unwrap();

        // offset_hi is as set
        assert_eq!(
            config.read_dword(cap_offset + REG_OFFSET_HI),
            (offset >> 32) as u32
        );

        // length_hi is as set
        assert_eq!(
            config.read_dword(cap_offset + REG_LENGTH_HI),
            (length >> 32) as u32
        );
    }

    pub fn check_cap_ro_fields(config: &mut PciConfigurationSpace, expected_rw_bits: &[u8]) {
        let (_, cap_offset) = config.capability_iter().last().unwrap();
        for (i, expect) in expected_rw_bits.iter().enumerate() {
            // First write 0 and record changed bits
            config.write_byte(cap_offset + i, 0x00);
            let mut changed_bits = !config.read_byte(cap_offset + i);

            // Then write 1
            config.write_byte(cap_offset + i, 0xFF);
            changed_bits &= config.read_byte(cap_offset + i);

            assert_eq!(changed_bits, *expect, "testing rw bits for byte {}", i);
        }
    }

    #[test]
    fn test_shm_cap() {
        let mut config = PciConfigurationSpace::new();

        let bar = 1;
        let shm_id = 2;
        let offs = 0x0001_0002_0003_0004;
        let len = 0x0050_0060_0070_0080;

        config
            .add_capability(&VirtioPciSharedMemoryCap::new(bar, shm_id, offs, len))
            .expect("adding cap failed");

        check_cap(
            &config,
            VIRTIO_CAP64_SIZE,
            VirtioPciCapType::SharedMemoryCfg,
            bar,
        );
        check_cap64_offs_len(&config, offs, len);

        // all fields RO
        check_cap_ro_fields(&mut config, &[0x00u8; VIRTIO_CAP64_SIZE])
    }
}
