// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::bar::{PciBar, PciBarIndex, PciBarType};
use crate::capability::{PciCapOffset, PciCapability};
use crate::registers::*;
use crate::utils::register_block::{
    CheckedRegisterBlockReader, CheckedRegisterBlockSetter, CheckedRegisterBlockWriter,
    ReadableRegisterBlock, RegisterBlockAccessValidator, RegisterBlockAutoImpl,
    RegisterBlockReader, RegisterBlockSetter, RegisterBlockWriteAccessValidator,
    RegisterBlockWriter, SettableRegisterBlock, WritableRegisterBlock, validate_bounds,
};
use crate::{Error, Result};

pub struct PciConfigurationSpace {
    registers: [u8; PCI_CONFIGURATION_SPACE_SIZE],
    writable_bits: [u8; PCI_CONFIGURATION_SPACE_SIZE],
    bars: [Option<PciBar>; NUM_BAR_REGS],
    last_cap_size: usize,
}

impl Default for PciConfigurationSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl PciConfigurationSpace {
    pub fn new() -> Self {
        let mut config = PciConfigurationSpace {
            registers: [0; PCI_CONFIGURATION_SPACE_SIZE],
            writable_bits: [0; PCI_CONFIGURATION_SPACE_SIZE],
            bars: [None; NUM_BAR_REGS],
            last_cap_size: 0,
        };

        // Vendor ID, R/O. Default to 0xFFFF as it means function not implemented.
        config.set_word(PCI_VENDOR_ID, 0xFFFF);

        config
    }

    // Shorter accessors for convenience.
    crate::impl_accessors! {
        load_arg(read, read_register, &mut [u8])
        load(read_byte, read_register_byte, u8)
        load(read_word, read_register_word, u16)
        load(read_dword, read_register_dword, u32)
        load_checked(read_checked, read_register_checked, &mut [u8], Result<()>)
        load(read_byte_checked, read_register_byte_checked, Result<u8>)
        load(read_word_checked, read_register_word_checked, Result<u16>)
        load(read_dword_checked, read_register_dword_checked, Result<u32>)
        store(set, set_register, &[u8])
        store(set_byte, set_register_byte, u8)
        store(set_word, set_register_word, u16)
        store(set_dword, set_register_dword, u32)
        store_checked(set_checked, set_register_checked, &[u8], Result<()>)
        store_checked(set_byte_checked, set_register_byte_checked, u8, Result<()>)
        store_checked(set_word_checked, set_register_word_checked, u16, Result<()>)
        store_checked(set_dword_checked, set_register_dword_checked, u32, Result<()>)
        // Write accessors
        store(write, write_register, &[u8])
        store(write_byte, write_register_byte, u8)
        store(write_word, write_register_word, u16)
        store(write_dword, write_register_dword, u32)
        store_checked(write_checked, write_register_checked, &[u8], Result<()>)
        store_checked(write_byte_checked, write_register_byte_checked, u8, Result<()>)
        store_checked(write_word_checked, write_register_word_checked, u16, Result<()>)
        store_checked(write_dword_checked, write_register_dword_checked, u32, Result<()>)
    }

    pub fn set_writable_byte(&mut self, offset: usize, bits: u8) {
        self.set_writable_bits(offset, &bits.to_ne_bytes())
    }

    pub fn set_writable_word(&mut self, offset: usize, bits: u16) {
        self.set_writable_bits(offset, &bits.to_ne_bytes())
    }

    pub fn set_writable_dword(&mut self, offset: usize, bits: u32) {
        self.set_writable_bits(offset, &bits.to_ne_bytes())
    }

    pub fn set_writable_bits(&mut self, offset: usize, bits: &[u8]) {
        self.writable_bits[offset..offset + bits.len()].copy_from_slice(bits)
    }

    pub fn add_writable_byte(&mut self, offset: usize, bits: u8) {
        self.add_writable_bits(offset, &bits.to_ne_bytes())
    }

    pub fn add_writable_word(&mut self, offset: usize, bits: u16) {
        self.add_writable_bits(offset, &bits.to_ne_bytes())
    }

    pub fn add_writable_dword(&mut self, offset: usize, bits: u32) {
        self.add_writable_bits(offset, &bits.to_ne_bytes())
    }

    pub fn add_writable_bits(&mut self, offset: usize, bits: &[u8]) {
        self.writable_bits[offset..offset + bits.len()]
            .iter_mut()
            .zip(bits)
            .for_each(|(cur, new)| *cur |= *new);
    }

    pub fn add_bar(&mut self, bar: PciBar) -> Result<()> {
        // Validate that the header type supports the BAR at index
        self.validate_header_support(&bar.index())?;

        validate_bar_size(&bar)?;
        validate_bar_region(&bar)?;

        let index = bar.index().into_inner();

        // Check that the current BAR is available
        if self
            .bars
            .get(index)
            .ok_or(Error::InvalidBarIndex { index })?
            .is_some()
        {
            return Err(Error::BarInUse { index });
        }

        // Check the previous BAR; if it is 64bit, the BAR is in use
        if let Some(prev_index) = index.checked_sub(1)
            && let Some(prev) = self.bars[prev_index]
            && let PciBarType::Memory64Bit(_) = prev.region_type()
        {
            return Err(Error::BarInUse { index });
        }

        let address = bar.address().unwrap_or(0);
        let bar_offset = PCI_BAR0 + (index * 4);

        let bar_mask = match bar.region_type() {
            PciBarType::Io => {
                self.add_writable_word(PCI_COMMAND, PCI_COMMAND_IO_SPACE_MASK);

                PCI_BAR_IO_BASE_ADDRESS_MASK
            }
            PciBarType::Memory32Bit(_) => {
                self.add_writable_word(PCI_COMMAND, PCI_COMMAND_MEM_SPACE_MASK);

                PCI_BAR_MEM_32_BASE_ADDRESS_MASK
            }
            PciBarType::Memory64Bit(_) => {
                let next_index = index + 1;

                // 64bit occupies the next BAR too; check if available
                if (self
                    .bars
                    .get(next_index)
                    .ok_or(Error::InvalidBarIndex { index })?)
                .is_some()
                {
                    return Err(Error::BarInUse { index });
                }

                // Set writable to bits [63:32] of the BAR
                let next_bar = PCI_BAR0 + (next_index * 4);
                self.set_writable_dword(next_bar, !((bar.size() - 1) >> 32) as u32);
                self.set_dword(next_bar, (address >> 32) as u32);

                self.add_writable_word(PCI_COMMAND, PCI_COMMAND_MEM_SPACE_MASK);

                // Lower part of the BAR
                PCI_BAR_MEM_32_BASE_ADDRESS_MASK
            }
        };

        // The driver determines BAR size by:
        // 1. Writing 0xFFFF_FFFF
        // 2. Reading the value (f.ex. 0xFFFF_1000). The high part of the register should be all
        //    1's
        // 3. By applying bitwise NOT and adding 1 we get the size
        //    f.ex !0xFFFF_1000 + 1 = 0x0000_7FFF + 1 = 0x0000_1000
        //
        // The 64bit BAR just extends to the next BAR register.
        //
        // Therefore, the writable bits are: !(size - 1)
        self.set_writable_dword(bar_offset, !(bar.size() - 1) as u32 & bar_mask);
        self.set_dword(
            bar_offset,
            (address as u32 & bar_mask) | bar.region_type().bits(),
        );

        self.bars[index] = Some(bar);
        Ok(())
    }

    pub fn get_bar(&self, index: PciBarIndex) -> Option<PciBar> {
        let index = index.into_inner();

        match self.bars.get(index)? {
            Some(bar) => {
                let mut bar = *bar;

                // Ensure the IO/MEM space is enabled
                let command = self.read_word(PCI_COMMAND);
                let (command_mask, bar_mask) = match bar.region_type() {
                    PciBarType::Io => (PCI_COMMAND_IO_SPACE_MASK, PCI_BAR_IO_BASE_ADDRESS_MASK),
                    _ => (PCI_COMMAND_MEM_SPACE_MASK, PCI_BAR_MEM_32_BASE_ADDRESS_MASK),
                };

                if command & command_mask == 0 {
                    // space disabled
                    return None;
                }

                let bar_offset = PCI_BAR0 + (index * 4);
                let mut address = u64::from(self.read_dword(bar_offset) & bar_mask);
                if let PciBarType::Memory64Bit(_) = bar.region_type() {
                    address |= u64::from(self.read_dword(bar_offset + 4)) << 32;
                }

                bar.set_address(Some(address));
                Some(bar)
            }
            None => None,
        }
    }

    pub fn add_capability<T: PciCapability>(&mut self, cap: &T) -> Result<PciCapOffset> {
        // Find the offsets
        let (mut cap_start, last_start) = if let Some((_, offset)) = self.capability_iter().last() {
            (
                offset + self.last_cap_size,
                // Iterator returns offset to data
                cap_start_from_data_start(offset),
            )
        } else {
            // Empty; set CAP pointer
            (PCI_DEVICE_SPECIFIC_START, 0)
        };

        // Align to the next dword
        cap_start = (cap_start + 3) & !3;
        let cap_data = cap_data_offset(cap_start);

        self.set_capability_data(cap, cap_data)?;

        // Set set the header
        // SAFETY: Safe because set_capability_data ensures the cap fits.
        self.set_byte(cap_id_offset(cap_start), cap.id().into());
        self.set_byte(cap_next_pointer_offset(cap_start), 0);

        self.last_cap_size = cap.size();

        // Update list pointers
        if last_start == 0 {
            // This is the first item
            self.set_byte(PCI_CAP_POINTER, PCI_DEVICE_SPECIFIC_START as u8);

            // Enable Capabilities List bit in status register
            let status = self.read_word(PCI_STATUS) | PCI_STATUS_CAP_LIST_MASK;
            self.set_word(PCI_STATUS, status);
        } else {
            self.set_byte(cap_next_pointer_offset(last_start), cap_start as u8);
        }

        // Skip CAP header
        Ok(cap_data)
    }

    pub fn update_capability<T: PciCapability>(
        &mut self,
        cap: &T,
        cap_start: PciCapOffset,
    ) -> Result<()> {
        // Make sure that the cap exists and that the cap_start matches
        self.capability_iter()
            .find(|(header, offset)| header.cap_id() == cap.id().into() && cap_start == *offset)
            .ok_or(Error::CapabilityNotFound {
                cap: cap.id().into(),
            })?;

        self.set_capability_data(cap, cap_start)
    }

    pub fn capability_iter(&self) -> PciCapabilitiesList<'_> {
        PciCapabilitiesList::new(self)
    }

    pub fn max_num_bars(&self) -> Result<usize> {
        let header_type = self.read_byte(PCI_HEADER_TYPE);
        match header_type {
            0 => Ok(PCI_TYPE0_NUM_BARS),
            1 => Ok(PCI_TYPE1_NUM_BARS),
            _ => Err(Error::UnsupportedHeader { header_type }),
        }
    }

    fn validate_header_support(&self, index: &PciBarIndex) -> Result<()> {
        let index = index.into_inner();
        match index < self.max_num_bars()? {
            true => Ok(()),
            false => Err(Error::InvalidBarIndex { index }),
        }
    }

    fn set_capability_data<T: PciCapability>(
        &mut self,
        cap: &T,
        cap_start: PciCapOffset,
    ) -> Result<()> {
        let cap_end = cap_end_offset(cap, cap_start)?;

        cap.registers(&mut self.registers[cap_start..cap_end]);
        cap.writable_bits(&mut self.writable_bits[cap_start..cap_end]);

        Ok(())
    }
}

// For read_register().
impl ReadableRegisterBlock for PciConfigurationSpace {
    fn registers(&self) -> &[u8] {
        &self.registers
    }
}

// For set_register().
impl SettableRegisterBlock for PciConfigurationSpace {
    fn registers_mut(&mut self) -> &mut [u8] {
        &mut self.registers
    }
}

// For write_register().
impl WritableRegisterBlock for PciConfigurationSpace {
    fn writable_bits(&self) -> &[u8] {
        &self.writable_bits
    }

    fn write_context(&mut self) -> (&mut [u8], &[u8]) {
        (&mut self.registers, &self.writable_bits)
    }
}

// Blankets, please.
impl RegisterBlockAutoImpl for PciConfigurationSpace {}

// For read/set_register_checked().
impl RegisterBlockAccessValidator for PciConfigurationSpace {
    fn validate_access(&self, offset: usize, data: &[u8]) -> Result<()> {
        let size = data.len();

        validate_access_size(size)?;
        validate_access_alignment(offset, size)?;
        validate_bounds(self.registers(), offset, data.len())
    }
}

// For write_register_checked().
impl RegisterBlockWriteAccessValidator for PciConfigurationSpace {
    fn validate_write_access(&self, offset: usize, data: &[u8]) -> Result<()> {
        self.validate_access(offset, data)?;
        validate_bounds(self.writable_bits(), offset, data.len())
    }
}

// Special handling for register: OOB access returns 0xFF.
impl CheckedRegisterBlockReader for PciConfigurationSpace {
    fn read_register_checked(&self, offset: usize, data: &mut [u8]) -> Result<()> {
        if let Err(e) = self.validate_access(offset, data) {
            match e {
                Error::AccessBounds { offset: _, size: _ } => {
                    data.fill(0xFF);
                    return Err(e);
                }
                error => return Err(error),
            }
        }

        self.read_register(offset, data);

        Ok(())
    }
}
impl CheckedRegisterBlockSetter for PciConfigurationSpace {}
impl CheckedRegisterBlockWriter for PciConfigurationSpace {
    fn write_register_checked(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        self.validate_write_access(offset, data)?;
        let (registers, writable_bits) = self.write_context();
        crate::utils::register_block::write(registers, writable_bits, offset, data);
        Ok(())
    }
}

/// Allows driver to enable Bus Master to device
pub fn allow_bus_master(config: &mut PciConfigurationSpace) {
    // SAFETY: PCI_COMMAND is within header
    config.add_writable_word(PCI_COMMAND, PCI_COMMAND_BUS_MASTER_MASK);
}

pub fn is_bus_master(config: &PciConfigurationSpace) -> bool {
    // SAFETY: PCI_COMMAND is within header
    let command = config.read_word(PCI_COMMAND);

    command & PCI_COMMAND_BUS_MASTER_MASK == PCI_COMMAND_BUS_MASTER_MASK
}

fn validate_access_alignment(offset: usize, size: usize) -> Result<()> {
    match offset % size {
        0 => Ok(()),
        _ => Err(Error::InvalidAccessAlignment {
            offset: offset as u64,
            size: size as u64,
        }),
    }
}

fn validate_access_size(size: usize) -> Result<()> {
    match size {
        1 | 2 | 4 => Ok(()),
        _ => Err(Error::InvalidIoSize { size }),
    }
}

fn validate_bar_size(bar: &PciBar) -> Result<()> {
    // Size must be power of two
    if !bar.size().is_power_of_two() {
        return Err(Error::InvalidBarSize { size: bar.size() });
    }

    // Minimum size
    let min_size = match bar.region_type() {
        PciBarType::Io => PCI_BAR_IO_MIN_SIZE,
        _ => PCI_BAR_MEM_MIN_SIZE,
    };

    if bar.size() < min_size {
        return Err(Error::InvalidBarSize { size: bar.size() });
    }

    // Maximum size
    let max_size = match bar.region_type() {
        PciBarType::Io => PCI_BAR_IO_MAX_SIZE,
        PciBarType::Memory32Bit(_) => PCI_BAR_MEM_32_MAX_SIZE,
        PciBarType::Memory64Bit(_) => PCI_BAR_MEM_64_MAX_SIZE,
    };

    if bar.size() > max_size {
        return Err(Error::InvalidBarSize { size: bar.size() });
    }

    Ok(())
}

fn validate_bar_region(bar: &PciBar) -> Result<()> {
    validate_bar_size(bar)?;

    if let Some(address) = bar.address() {
        let size = bar.size();

        if address == 0 {
            return Err(Error::InvalidBarAddress { address });
        }

        // BAR address must be naturally aligned
        if address % bar.size() != 0 {
            return Err(Error::InvalidBarAlignment { address, size });
        }

        // Ensure region with size can be placed at address
        let end_address = address
            .checked_add(size)
            .ok_or(Error::BarRegionOverflow { address, size })?;

        if let PciBarType::Memory32Bit(_) | PciBarType::Io = bar.region_type()
            && end_address > u64::from(u32::MAX)
        {
            return Err(Error::BarRegionOverflow { address, size });
        }
    }

    Ok(())
}

fn cap_end_offset<T: PciCapability>(cap: &T, cap_start: PciCapOffset) -> Result<PciCapOffset> {
    let Some(cap_end) = cap_start.checked_add(cap.size()) else {
        return Err(Error::ConfigurationSpaceBounds {
            limit: PCI_CONFIGURATION_SPACE_SIZE,
        });
    };

    if cap_end > PCI_DEVICE_SPECIFIC_END {
        return Err(Error::DeviceAreaBounds {
            offset: cap_end,
            limit: PCI_DEVICE_SPECIFIC_END,
        });
    }

    Ok(cap_end)
}

fn cap_id_offset(cap_start: PciCapOffset) -> PciCapOffset {
    cap_start
}

fn cap_next_pointer_offset(cap_start: PciCapOffset) -> PciCapOffset {
    cap_start + PCI_CAP_HEADER_NEXT
}

fn cap_data_offset(cap_start: PciCapOffset) -> PciCapOffset {
    cap_start + PCI_CAP_DATA_START
}

fn cap_start_from_data_start(data_start: PciCapOffset) -> PciCapOffset {
    data_start - PCI_CAP_DATA_START
}

pub struct PciCapabilitiesList<'a> {
    config: &'a PciConfigurationSpace,
    next_pointer: PciCapOffset,
}

impl<'a> PciCapabilitiesList<'a> {
    pub fn new(config: &'a PciConfigurationSpace) -> Self {
        Self {
            config,
            next_pointer: PCI_CAP_POINTER,
        }
    }

    fn next_cap(config: &PciConfigurationSpace, pointer: PciCapOffset) -> PciCapOffset {
        let next = usize::from(config.read_byte(pointer) & !(PCI_CAP_POINTER_RSVD_MASK));
        // Must be within device specific area
        if next < PCI_DEVICE_SPECIFIC_START {
            return 0;
        }
        next
    }
}

impl Iterator for PciCapabilitiesList<'_> {
    type Item = (PciCapHeader, PciCapOffset);

    fn next(&mut self) -> Option<Self::Item> {
        let next_cap = Self::next_cap(self.config, self.next_pointer);
        if next_cap != 0 {
            let header = PciCapHeader::from(&self.config.registers[next_cap..]);
            let next_pointer: usize = header.next_pointer().into();

            if cap_next_pointer_offset(next_pointer) == self.next_pointer {
                // Avoid recursion.
                return None;
            }

            self.next_pointer = cap_next_pointer_offset(next_cap);

            // Return header and offset to cap data
            Some((header, cap_data_offset(next_cap)))
        } else {
            None
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PciCapHeader {
    pub cap_id: u8,
    pub next_pointer: u8,
}

impl PciCapHeader {
    pub fn new(cap_id: u8, next_pointer: u8) -> Self {
        Self {
            cap_id,
            next_pointer,
        }
    }

    pub fn cap_id(&self) -> u8 {
        self.cap_id
    }

    pub fn next_pointer(&self) -> u8 {
        self.next_pointer
    }
}

impl From<&[u8]> for PciCapHeader {
    fn from(registers: &[u8]) -> Self {
        Self {
            // PCI spec: the bottom two bits must be masked by software
            cap_id: registers[PCI_CAP_HEADER_ID],
            next_pointer: registers[PCI_CAP_HEADER_NEXT],
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bar::PciBarPrefetchable::NotPrefetchable;
    use crate::capability::{PciCapability, PciCapabilityId};

    use super::*;

    #[test]
    fn test_access_bounds() {
        let mut config = PciConfigurationSpace::new();

        let mut offset = PCI_CONFIGURATION_SPACE_SIZE;
        let mut data = [0u8; 8];
        assert!(
            matches!(
                config.read_checked(offset, &mut data[0..1]),
                Err(Error::AccessBounds { offset: _, size: _ })
            ),
            "unexpected config read return"
        );
        // Data must be set to 0xFF on OoB reads
        assert_eq!(data[0], 0xFF);

        assert!(
            matches!(
                config.write_checked(offset, &[0; 1]),
                Err(Error::AccessBounds { offset: _, size: _ })
            ),
            "unexpected config write return"
        );

        offset = PCI_CONFIGURATION_SPACE_SIZE - 1;
        assert_eq!(config.read_checked(offset, &mut data[0..1]).is_ok(), true);
        assert_eq!(config.write_checked(offset, &[0; 1]).is_ok(), true);
    }

    #[test]
    fn test_access_size() {
        let mut config = PciConfigurationSpace::new();

        let input = [0xAAu8; 9];
        let mut output = [0u8; 9];
        for size in [0, 3, 5, 8, 9] {
            assert!(
                matches!(
                    config.read_checked(0, &mut output[0..size]),
                    Err(Error::InvalidIoSize { size: _ })
                ),
                "unexpected config read return"
            );

            assert!(
                matches!(
                    config.write_checked(0, &input[0..size]),
                    Err(Error::InvalidIoSize { size: _ })
                ),
                "unexpected config write return"
            );
        }

        for size in [1, 2, 4] {
            assert!(config.read_checked(0, &mut output[0..size]).is_ok());
            assert!(config.write_checked(0, &input[0..size]).is_ok());
        }
    }

    #[test]
    fn test_access_alignment() {
        let mut config = PciConfigurationSpace::new();

        let mut data = [0xAA; 8];
        for (offset, size) in [(1, 2), (2, 4)] {
            assert!(
                matches!(
                    config.read_checked(offset, &mut data[0..size]),
                    Err(Error::InvalidAccessAlignment { offset: _, size: _ })
                ),
                "unexpected config read return"
            );

            assert!(
                matches!(
                    config.write_checked(offset, &data[0..size]),
                    Err(Error::InvalidAccessAlignment { offset: _, size: _ })
                ),
                "unexpected config write return"
            );
        }
    }

    #[test]
    fn test_set_writable_bits() {
        let mut config = PciConfigurationSpace::new();

        config.set_writable_bits(5, &[0xFF, 0xA, 0x1]);
        assert_eq!(
            u32::from_ne_bytes(config.writable_bits[4..8].try_into().unwrap()),
            0x010AFF00
        );
    }

    #[test]
    fn test_set_writable_byte() {
        let mut config = PciConfigurationSpace::new();

        config.set_writable_byte(11, 0xAF);
        assert_eq!(config.writable_bits[11], 0xAF);
    }

    #[test]
    fn test_set_writable_word() {
        let mut config = PciConfigurationSpace::new();

        config.set_writable_word(12, 0xABCD);
        assert_eq!(
            u16::from_ne_bytes(config.writable_bits[12..14].try_into().unwrap()),
            0xABCD
        );
    }

    #[test]
    fn test_set_writable_dword() {
        let mut config = PciConfigurationSpace::new();

        config.set_writable_dword(12, 0xABCDEF01);
        assert_eq!(
            u32::from_ne_bytes(config.writable_bits[12..16].try_into().unwrap()),
            0xABCDEF01
        );
    }

    #[test]
    fn test_add_writable_bits() {
        let mut config = PciConfigurationSpace::new();

        config.writable_bits[5..8].copy_from_slice(&[0x1, 0x2, 0x7]);
        config.add_writable_bits(5, &[0x2, 0x1, 0x8]);

        assert_eq!(
            u32::from_ne_bytes(config.writable_bits[4..8].try_into().unwrap()),
            0x0F030300
        );
    }

    #[test]
    fn test_add_writable_byte() {
        let mut config = PciConfigurationSpace::new();

        config.writable_bits[11] = 0x7;
        config.add_writable_byte(11, 0x8);
        assert_eq!(config.writable_bits[11], 0x0F);
    }

    #[test]
    fn test_add_writable_word() {
        let mut config = PciConfigurationSpace::new();

        config.writable_bits[5..8].copy_from_slice(&[0x1, 0x2, 0x7]);
        config.add_writable_word(6, 0xA801);

        assert_eq!(
            u16::from_ne_bytes(config.writable_bits[6..8].try_into().unwrap()),
            0xAF03
        );
    }

    #[test]
    fn test_add_writable_dword() {
        let mut config = PciConfigurationSpace::new();

        config.writable_bits[5..8].copy_from_slice(&[0x1, 0x2, 0x7]);
        config.add_writable_dword(4, 0xA801_0000);

        assert_eq!(
            u32::from_ne_bytes(config.writable_bits[4..8].try_into().unwrap()),
            0xAF030100
        );
    }

    #[test]
    fn test_add_bar() {
        let mut config = PciConfigurationSpace::new();
        let bar1 = PciBar::new(
            None,
            PCI_BAR_MEM_MIN_SIZE,
            PciBarIndex::try_from(0).unwrap(),
            PciBarType::Memory32Bit(NotPrefetchable),
        );
        config.add_bar(bar1).expect("adding bar1 failed");

        let bar2 = PciBar::new(
            None,
            PCI_BAR_IO_MIN_SIZE,
            PciBarIndex::try_from(1).unwrap(),
            PciBarType::Io,
        );
        config.add_bar(bar2).expect("adding bar2 failed");

        let bar3 = PciBar::new(
            None,
            PCI_BAR_MEM_MIN_SIZE,
            PciBarIndex::try_from(2).unwrap(),
            PciBarType::Io,
        );
        config.add_bar(bar3).expect("adding bar3 failed");
    }

    #[test]
    fn test_add_bar_in_use() {
        let mut config = PciConfigurationSpace::new();
        let bar1 = PciBar::new(
            None,
            PCI_BAR_MEM_MIN_SIZE,
            PciBarIndex::try_from(0).unwrap(),
            PciBarType::Memory64Bit(NotPrefetchable),
        );
        config.add_bar(bar1).expect("adding bar1 failed");

        let bar2 = PciBar::new(
            None,
            PCI_BAR_MEM_MIN_SIZE,
            PciBarIndex::try_from(0).unwrap(),
            PciBarType::Memory32Bit(NotPrefetchable),
        );
        assert!(config.add_bar(bar2).is_err());
    }

    #[test]
    fn test_get_bar() {
        let mut config = PciConfigurationSpace::new();
        assert!(config.get_bar(PciBarIndex::default()).is_none());

        let bar1 = PciBar::new(
            None,
            PCI_BAR_MEM_MIN_SIZE,
            PciBarIndex::default(),
            PciBarType::Memory32Bit(NotPrefetchable),
        );
        config.add_bar(bar1).expect("adding bar1 failed");

        // Interface disabled.
        assert!(config.get_bar(PciBarIndex::default()).is_none());

        // Interface enabled.
        config.set_word(PCI_COMMAND, PCI_COMMAND_MEM_SPACE_MASK);
        assert!(config.get_bar(PciBarIndex::default()).is_some());
    }

    #[test]
    fn test_max_num_bars() {
        let mut config = PciConfigurationSpace::new();

        config.registers[PCI_HEADER_TYPE] = 0;
        assert_eq!(config.max_num_bars().unwrap(), PCI_TYPE0_NUM_BARS);

        config.registers[PCI_HEADER_TYPE] = 1;
        assert_eq!(config.max_num_bars().unwrap(), PCI_TYPE1_NUM_BARS);

        config.registers[PCI_HEADER_TYPE] = 3;
        assert!(config.max_num_bars().is_err());
    }

    #[test]
    fn test_empty_cap_list() {
        let config = PciConfigurationSpace::new();
        let mut list = PciCapabilitiesList::new(&config);
        assert_eq!(list.next(), None);
    }

    #[test]
    fn test_invalid_cap_list() {
        let mut config = PciConfigurationSpace::new();
        config.registers[PCI_CAP_POINTER] = 0x4;
        config.registers[0x4] = PciCapabilityId::DebugPort.into();
        config.registers[0x5] = 0x40;
        let mut list = PciCapabilitiesList::new(&config);
        // Must be within 0x40-0xFF -> None
        assert!(list.next().is_none());
    }

    #[test]
    fn test_cap_list_recursion() {
        let mut config = PciConfigurationSpace::new();
        config.registers[PCI_CAP_POINTER] = 0x40;
        // Point to self
        config.registers[0x40] = PciCapabilityId::DebugPort.into();
        config.registers[0x41] = 0x40;

        let mut list = PciCapabilitiesList::new(&config);
        assert!(list.next().is_some());
        // Detects recursion -> None
        assert!(list.next().is_none());
    }

    #[test]
    fn test_cap_list_unaligned_pointer() {
        let mut config = PciConfigurationSpace::new();
        config.registers[PCI_CAP_POINTER] = 0x43;
        let expected = PciCapHeader::from(&config.registers[0x40..]);

        let mut list = PciCapabilitiesList::new(&config);
        assert_eq!(list.next(), Some((expected, 0x42)));
    }

    fn add_dummy_caps(config: &mut PciConfigurationSpace) {
        config.registers[PCI_CAP_POINTER] = 0x40;
        config.registers[0x40] = PciCapabilityId::NullCap.into();
        config.registers[0x41] = 0x48;
        config.registers[0x48] = PciCapabilityId::VendorSpecific.into();
    }

    #[test]
    fn test_cap_list() {
        let mut config = PciConfigurationSpace::new();
        add_dummy_caps(&mut config);

        let mut list = PciCapabilitiesList::new(&config).enumerate();

        let mut expected = PciCapHeader::from(&config.registers[0x40..]);
        assert_eq!(list.next(), Some((0, (expected, 0x42))));
        expected = PciCapHeader::from(&config.registers[0x48..]);
        assert_eq!(list.next(), Some((1, (expected, 0x4A))));
    }

    struct Cap1 {
        regs: [u8; 6],
        writable_bits: [u8; 6],
        size: Option<usize>,
        _offset: Option<PciCapOffset>,
    }

    impl Cap1 {
        fn new(regs: Option<[u8; 6]>) -> Self {
            Self {
                regs: regs.unwrap_or([0u8; 6]),
                writable_bits: regs.unwrap_or([0u8; 6]),
                size: None,
                _offset: None,
            }
        }
    }

    impl PciCapability for Cap1 {
        fn id(&self) -> PciCapabilityId {
            PciCapabilityId::VendorSpecific
        }

        fn size(&self) -> usize {
            self.size.unwrap_or(self.regs.len())
        }

        fn registers(&self, data: &mut [u8]) {
            assert_eq!(self.size(), data.len());
            data.copy_from_slice(&self.regs)
        }
        fn writable_bits(&self, data: &mut [u8]) {
            assert_eq!(self.size(), data.len());
            data.copy_from_slice(&self.writable_bits)
        }
    }

    fn validate_cap(
        config: &PciConfigurationSpace,
        cap: &Cap1,
        cap_start: PciCapOffset,
        next_start: PciCapOffset,
    ) {
        assert_eq!(config.registers[cap_start], cap.id().into());
        assert_eq!(config.registers[cap_start + 1], next_start as u8);

        let data = cap_start + 2;
        assert_eq!(config.registers[data..data + cap.size()], cap.regs);
        assert_eq!(
            config.writable_bits[data..data + cap.size()],
            cap.writable_bits
        );
    }

    #[test]
    fn test_add_capability() {
        let mut config = PciConfigurationSpace::new();
        let cap = Cap1::new(Some([0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]));
        let another = Cap1::new(Some([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]));

        assert_eq!(config.registers[PCI_STATUS], 0);

        // Add first cap
        let cap_start = PCI_DEVICE_SPECIFIC_START;
        assert_eq!(config.add_capability(&cap), Ok(cap_start + 2));
        validate_cap(&config, &cap, cap_start, 0);

        assert_eq!(config.registers[PCI_STATUS], PCI_STATUS_CAP_LIST_MASK as u8);
        assert_eq!(config.registers[PCI_CAP_POINTER], cap_start as u8);

        // Add second cap; should be aligned to the next dword
        assert!(cap.size() % 4 != 0);
        let second_start = cap_start + cap.size() + 2;
        assert_eq!(config.add_capability(&another), Ok(second_start + 2));
        validate_cap(&config, &another, second_start, 0);

        // Make sure the first cap points to the second
        validate_cap(&config, &cap, cap_start, second_start);
        // ... and cap pointer still points to the first
        assert_eq!(config.registers[PCI_CAP_POINTER], cap_start as u8);
    }

    #[test]
    fn test_add_capability_no_space() {
        let mut config = PciConfigurationSpace::new();
        let first = Cap1::new(None);
        // 2 bytes for both caps
        let hdrs = 4;
        let second = InvalidCap {
            // 1 byte too large
            size: PCI_DEVICE_SPECIFIC_END - PCI_DEVICE_SPECIFIC_START - first.size() - hdrs + 1,
        };
        assert!(config.add_capability(&first).is_ok());
        assert!(config.add_capability(&second).is_err());
    }

    #[test]
    fn test_update_capability_not_found() {
        let mut config = PciConfigurationSpace::new();
        let cap = Cap1::new(None);

        // No cap -> not found
        assert!(config.update_capability(&cap, 0x42).is_err());

        add_dummy_caps(&mut config);

        // Offset does not match - not found.
        assert!(config.update_capability(&cap, 0x42).is_err());
    }

    struct InvalidCap {
        size: usize,
    }

    impl PciCapability for InvalidCap {
        fn id(&self) -> PciCapabilityId {
            PciCapabilityId::DebugPort
        }
        fn size(&self) -> usize {
            self.size
        }
        fn registers(&self, _registers: &mut [u8]) {}
        fn writable_bits(&self, _writable_bits: &mut [u8]) {}
    }

    #[test]
    fn test_update_capability_invalid_size() {
        let mut config = PciConfigurationSpace::new();
        let cap = InvalidCap {
            size: usize::max_value(),
        };
        config.registers[PCI_CAP_POINTER] = 0x40;
        config.registers[0x40] = cap.id().into();

        assert_eq!(
            config.update_capability(&cap, 0x42),
            Err(Error::ConfigurationSpaceBounds {
                limit: PCI_CONFIGURATION_SPACE_SIZE
            })
        );
    }

    #[test]
    fn test_update_capability_device_area_bounds() {
        let mut config = PciConfigurationSpace::new();
        let cap = InvalidCap {
            size: PCI_DEVICE_SPECIFIC_END,
        };
        config.registers[PCI_CAP_POINTER] = 0x40;
        config.registers[0x40] = cap.id().into();

        assert_eq!(
            config.update_capability(&cap, 0x42),
            Err(Error::DeviceAreaBounds {
                offset: 0x42 + PCI_DEVICE_SPECIFIC_END,
                limit: PCI_DEVICE_SPECIFIC_END
            })
        );
    }

    #[test]
    fn test_update_capability() {
        let mut config = PciConfigurationSpace::new();
        let expected = [0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA];
        let cap = Cap1::new(Some(expected.clone()));

        // Add cap
        config.registers[PCI_CAP_POINTER] = 0x40;
        config.registers[0x40] = cap.id().into();

        // Set cap data and writable bits
        let data = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        config.registers[0x42..0x42 + data.len()].copy_from_slice(&data);
        config.writable_bits[0x42..0x42 + data.len()].copy_from_slice(&data);

        assert!(config.update_capability(&cap, 0x42).is_ok());
        assert_eq!(
            &config.registers[0x42..0x42 + expected.len()],
            expected.as_slice()
        );
        assert_eq!(
            &config.writable_bits[0x42..0x42 + expected.len()],
            expected.as_slice()
        );
    }

    fn find_cap(
        config: &PciConfigurationSpace,
        id: PciCapabilityId,
    ) -> Option<(PciCapHeader, usize)> {
        config
            .capability_iter()
            .find(|(header, _)| header.cap_id() == id.into())
    }

    #[test]
    fn test_capability_iter() {
        let mut config = PciConfigurationSpace::new();

        // Find from empty
        assert_eq!(find_cap(&config, PciCapabilityId::NullCap), None);

        add_dummy_caps(&mut config);

        // Should find the caps
        assert_eq!(
            find_cap(&config, PciCapabilityId::NullCap),
            Some((
                PciCapHeader {
                    cap_id: 0,
                    next_pointer: 0x48
                },
                0x42
            ))
        );
        assert_eq!(
            find_cap(&config, PciCapabilityId::VendorSpecific),
            Some((
                PciCapHeader {
                    cap_id: PciCapabilityId::VendorSpecific.into(),
                    next_pointer: 0
                },
                0x4A
            ))
        );
    }
}
