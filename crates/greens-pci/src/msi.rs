// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::cmp;
use std::mem;

use crate::capability::{PciCapOffset, PciCapability, PciCapabilityId};
use crate::config_handler::PciConfigurationSpaceIoHandler;
use crate::configuration_space::{is_bus_master, PciConfigurationSpace};
use crate::function::{PciHandlerResult, PciInterruptConfigEvent};
use crate::utils::{
    access_data_window, offset_within_range, range_overlaps, read, read_dword, read_word,
    set_dword, set_word, write,
};
use crate::{Error, PciMsiMessage, Result};

pub type PciMsiVector = usize;

#[derive(Debug, PartialEq)]
pub enum PciMsiGenerationResult {
    Masked,
    Generated(PciMsiMessage),
}

pub trait PciMsiMessageSource {
    /// Checks if `vector` is valid for the MSI message source.
    ///
    /// Returns `true` for a valid `vector`, otherwise returns `false`.
    fn is_valid_vector(&mut self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool;

    /// Checks if the MSI message source is enabled.
    ///
    /// Returns `true` when the source is enabled. Otherwise returns `false`.
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool;

    /// Checks if `vector` is masked.
    ///
    /// Returns `true` when the `vector` or the whole function is masked. Otherwise returns `false`.
    fn is_masked(&mut self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool;

    /// Checks if `vector` is pending.
    ///
    /// Returns `true` when the `vector` is pending. Otherwise returns `false`
    fn is_pending(&mut self, vector: PciMsiVector) -> bool;

    /// Set pending bit for `vector`.
    ///
    /// Returns `Ok(())` when the `vector` is set pending. Otherwise returns `Err(Error)`.
    fn set_pending_bit(
        &mut self,
        config: &mut PciConfigurationSpace,
        vector: PciMsiVector,
        pending: bool,
    ) -> Result<()>;

    /// Create a new message for `vector`.
    ///
    /// Returns `Ok(message)` on success, otherwise `Err(Error)`.
    fn get_message_for(&mut self, vector: PciMsiVector) -> Result<PciMsiMessage>;

    /// Attempts to generate an MSI message for `vector`.
    ///
    /// Checks all conditions (enabled bit, vector and function masks, etc.) and if an MSI
    /// generation is successful, returns `Ok(PciMsiGenerationResult::Generated(PciMessage))`. If
    /// the vector is masked, the method is responsible for setting the pending bit and returning
    /// `Ok(Masked)`.
    /// generated, returns a new `Ok(Generated(PciMessage))` with configured address and data. This method is
    /// responsible for handling pending bits, if applicable. If a message cannot be generated, it
    /// returns corresponding error.
    fn try_generate_message(
        &mut self,
        config: &mut PciConfigurationSpace,
        vector: PciMsiVector,
    ) -> Result<PciMsiGenerationResult> {
        // Required for MSI writes.
        if !is_bus_master(config) {
            return Err(Error::NotBusMaster);
        }

        // MSI source must be enabled.
        if !self.is_enabled(config) {
            return Err(Error::NoMsi);
        }

        if self.is_masked(config, vector) {
            self.set_pending_bit(config, vector, true)?;
            return Ok(PciMsiGenerationResult::Masked);
        }

        self.set_pending_bit(config, vector, false)?;
        let message = self.get_message_for(vector)?;

        Ok(PciMsiGenerationResult::Generated(message))
    }
}

const MESSAGE_CONTROL: usize = 0;
const MESSAGE_CONTROL_VECTOR_MASKING_SHIFT: usize = 8;
const MESSAGE_CONTROL_VECTOR_MASKING: u16 = 1 << MESSAGE_CONTROL_VECTOR_MASKING_SHIFT;

const MESSAGE_CONTROL_64BIT_SHIFT: usize = 7;
const MESSAGE_CONTROL_64BIT: u16 = 1 << MESSAGE_CONTROL_64BIT_SHIFT;

const MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT: usize = 4;
const MESSAGE_CONTROL_MULTIPLE_MSG_EN: u16 = 0b111 << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT;

const MESSAGE_CONTROL_MULTIPLE_MSG_CAP_SHIFT: usize = 1;
#[allow(dead_code)]
const MESSAGE_CONTROL_MULTIPLE_MSG_CAP: u16 = 0b111 << MESSAGE_CONTROL_MULTIPLE_MSG_CAP_SHIFT;

const MESSAGE_CONTROL_MSI_EN: u16 = 1;

const MESSAGE_ADDRESS: usize = 2;
const MESSAGE_ADDRESS_MASK: u32 = !0b11;
const MESSAGE_ADDRESS_UPPER: usize = MESSAGE_ADDRESS + 4;
const MESSAGE_ADDRESS_UPPER_MASK: u32 = !0;

const MESSAGE_DATA_32BIT: usize = MESSAGE_ADDRESS + 4;
const MESSAGE_DATA_64BIT: usize = MESSAGE_ADDRESS_UPPER + 4;
const MESSAGE_DATA_MASK: u16 = !0;

// Message data 16bits + reserved 16bits
const MASK_BITS_32BIT: usize = MESSAGE_DATA_32BIT + 4;
const MASK_BITS_64BIT: usize = MESSAGE_DATA_64BIT + 4;
const MASK_BITS_MASK: u32 = !0;

const PENDING_BITS_32BIT: usize = MASK_BITS_32BIT + 4;
const PENDING_BITS_64BIT: usize = MASK_BITS_64BIT + 4;
const PENDING_BITS_MASK: u32 = !0;

const MSI_REGS_MAX_SIZE: usize = PENDING_BITS_64BIT + 4;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u16)]
pub enum MsiMultipleMessage {
    One = 0b000,
    Two = 0b001,
    Four = 0b010,
    Eight = 0b011,
    Sixteen = 0b100,
    ThirtyTwo = 0b101,
}

impl TryFrom<u16> for MsiMultipleMessage {
    type Error = Error;

    fn try_from(value: u16) -> Result<Self> {
        if value > Self::ThirtyTwo as u16 {
            return Err(Error::InvalidMultipleMessageValue { value });
        }

        // SAFETY: safe because enum is linear and the upper bound is checked above
        Ok(unsafe { mem::transmute::<u16, MsiMultipleMessage>(value) })
    }
}

impl MsiMultipleMessage {
    pub fn encode(&self) -> u16 {
        *self as u16
    }

    pub fn decode(val: u16) -> Option<Self> {
        let val = val & 0b111;

        Self::try_from(val).ok()
    }

    pub fn to_capable(&self) -> u16 {
        self.encode() << MESSAGE_CONTROL_MULTIPLE_MSG_CAP_SHIFT
    }

    pub fn from_capable(message_control: u16) -> Option<Self> {
        Self::decode(message_control >> MESSAGE_CONTROL_MULTIPLE_MSG_CAP_SHIFT)
    }

    pub fn to_enable(&self) -> u16 {
        self.encode() << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT
    }

    pub fn from_enable(message_control: u16) -> Option<Self> {
        Self::decode(message_control >> MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT)
    }

    pub fn max_vectors(&self) -> u8 {
        1 << (*self as u8)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u16)]
pub enum MsiAddressWidth {
    Address32Bit = 0,
    Address64Bit = MESSAGE_CONTROL_64BIT,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u16)]
pub enum MsiPerVectorMasking {
    Disabled = 0,
    Enabled = MESSAGE_CONTROL_VECTOR_MASKING,
}

// const MESSAGE_ADDRESS: usize = 2;
// const MESSAGE_DATA_32BIT: usize = 4;

pub struct PciMsiCapability {
    registers: [u8; MSI_REGS_MAX_SIZE],
}

impl PciMsiCapability {
    pub fn new(
        width: MsiAddressWidth,
        vectors: MsiMultipleMessage,
        per_vector_masking: MsiPerVectorMasking,
    ) -> Self {
        let mut cap = Self {
            registers: [0u8; MSI_REGS_MAX_SIZE],
        };

        let control = width as u16 | vectors.to_capable() | per_vector_masking as u16;
        set_word(&mut cap.registers, MESSAGE_CONTROL, control).unwrap();

        cap
    }

    pub fn read(&self, offset: usize, data: &mut [u8]) -> Result<()> {
        read(&self.registers, offset, data)
    }

    pub fn write(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        let mut writable = [0u8; MSI_REGS_MAX_SIZE];

        self.writable_bits(&mut writable);
        write(&mut self.registers, &writable, offset, data)?;

        if range_overlaps(offset, data.len(), MESSAGE_CONTROL, 2) {
            let mut control = read_word(&self.registers, MESSAGE_CONTROL)?;

            if let Some(vectors_en) = MsiMultipleMessage::from_enable(control) {
                if vectors_en.max_vectors() >= self.num_capable_vectors() {
                    // Invalid multiple messages, default to 0
                    control &= !MESSAGE_CONTROL_MULTIPLE_MSG_EN;
                    set_word(&mut self.registers, MESSAGE_CONTROL, control)?;
                }
            } else {
                // Illegal multiple messages, default to 0
                control &= !MESSAGE_CONTROL_MULTIPLE_MSG_EN;
                set_word(&mut self.registers, MESSAGE_CONTROL, control)?;
            }
        }
        if let Some(mask_bits) = self.mask_bits_offset() {
            if range_overlaps(offset, data.len(), mask_bits, 4) {
                todo!();
            }
        }

        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        read_word(&self.registers, MESSAGE_CONTROL).unwrap() & MESSAGE_CONTROL_MSI_EN
            == MESSAGE_CONTROL_MSI_EN
    }

    pub fn is_64bit(&self) -> bool {
        let mask = MsiAddressWidth::Address64Bit as u16;
        read_word(&self.registers, MESSAGE_CONTROL).unwrap() & mask == mask
    }

    pub fn per_vector_masking_enabled(&self) -> bool {
        let mask = MsiPerVectorMasking::Enabled as u16;
        (read_word(&self.registers, MESSAGE_CONTROL).unwrap() & mask) == mask
    }

    pub fn capable_vectors(&self) -> MsiMultipleMessage {
        let Ok(control) = read_word(&self.registers, MESSAGE_CONTROL) else {
            return MsiMultipleMessage::One;
        };

        MsiMultipleMessage::from_capable(control).unwrap_or(MsiMultipleMessage::One)
    }

    pub fn num_capable_vectors(&self) -> u8 {
        self.capable_vectors().max_vectors()
    }

    pub fn enabled_vectors(&self) -> MsiMultipleMessage {
        let Ok(control) = read_word(&self.registers, MESSAGE_CONTROL) else {
            return MsiMultipleMessage::One;
        };
        MsiMultipleMessage::from_enable(control).unwrap_or(MsiMultipleMessage::One)
    }

    pub fn num_enabled_vectors(&self) -> u8 {
        self.enabled_vectors().max_vectors()
    }

    fn get_message_data_for(&self, vector: u8) -> Option<u16> {
        if !self.is_valid_vector(vector) {
            return None;
        }

        let Ok(message_data) = read_word(&self.registers, self.message_data_offset()) else {
            return None;
        };

        // Bit combination of enabled vectors determines the number of bits the function is allowed
        // to modify, f.ex. 0b010 -> 2bits. Unfortunately the specification does not specify how,
        // although it is implied that the modifiable bits are set to 0 by the system software.
        //
        // For simplicity, just replace the base data bits and insert the vector.
        let modifiable_mask = (1 << self.enabled_vectors() as u16) - 1;

        Some((message_data & !modifiable_mask) | vector as u16)
    }

    pub fn get_message_for(&self, vector: u8) -> Result<PciMsiMessage> {
        let mut address: u64 = read_dword(&self.registers, MESSAGE_ADDRESS).unwrap() as u64;
        if self.is_64bit() {
            address |= (read_dword(&self.registers, MESSAGE_ADDRESS_UPPER).unwrap() as u64) << 32;
        }

        let Some(data) = self.get_message_data_for(vector) else {
            return Err(Error::InvalidMsiVector { vector });
        };

        Ok(PciMsiMessage {
            address,
            data: data as u32,
        })
    }

    pub fn set_pending_bit(&mut self, vector: u8) -> Result<()> {
        let _ = vector;
        todo!()
    }

    pub fn clear_pending_bit(&mut self, vector: u8) -> Result<()> {
        let _ = vector;
        todo!()
    }

    pub fn is_masked(&self, vector: u8) -> Result<bool> {
        let _ = vector;
        todo!()
    }

    fn is_valid_vector(&self, vector: u8) -> bool {
        vector < self.num_enabled_vectors()
    }

    fn message_data_offset(&self) -> usize {
        match self.is_64bit() {
            true => MESSAGE_DATA_64BIT,
            false => MESSAGE_DATA_32BIT,
        }
    }

    fn mask_bits_offset(&self) -> Option<usize> {
        if self.per_vector_masking_enabled() {
            let offset = match self.is_64bit() {
                true => MASK_BITS_64BIT,
                false => MASK_BITS_32BIT,
            };
            return Some(offset);
        }
        None
    }

    fn pending_bits_offset(&self) -> Option<usize> {
        if self.per_vector_masking_enabled() {
            let offset = match self.is_64bit() {
                true => PENDING_BITS_64BIT,
                false => PENDING_BITS_32BIT,
            };
            return Some(offset);
        }
        None
    }
}

impl PciCapability for PciMsiCapability {
    fn id(&self) -> PciCapabilityId {
        PciCapabilityId::Msi
    }

    fn size(&self) -> usize {
        // 2 (control) + 4 (message address) + 2 (message data)
        let mut size = 8;
        if self.per_vector_masking_enabled() {
            // 2 (reserved) + 4 (mask bits) + 4 (pending bits)
            size += 10;
        }

        if self.is_64bit() {
            // 4 (upper message address)
            size += 4;
        }

        size
    }

    fn registers(&self, registers: &mut [u8]) {
        let size = cmp::min(self.size(), registers.len());
        registers.copy_from_slice(&self.registers[0..size])
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        // FIXME: check the size
        // let size = cmp::min(self.size(), writable_bits.len());

        // Control register: multiple messages enable and MSI enable
        set_word(
            writable_bits,
            MESSAGE_CONTROL,
            MESSAGE_CONTROL_MULTIPLE_MSG_EN | MESSAGE_CONTROL_MSI_EN,
        )
        .unwrap();

        // Message address: bits [64:02] are writable
        set_dword(writable_bits, MESSAGE_ADDRESS, MESSAGE_ADDRESS_MASK).unwrap();
        if self.is_64bit() {
            set_dword(
                writable_bits,
                MESSAGE_ADDRESS_UPPER,
                MESSAGE_ADDRESS_UPPER_MASK,
            )
            .unwrap();
        }

        // Message data is writable
        set_word(writable_bits, self.message_data_offset(), MESSAGE_DATA_MASK).unwrap();

        if let Some(offset) = self.mask_bits_offset() {
            // Mask bits are writable
            set_dword(writable_bits, offset, MASK_BITS_MASK).unwrap();
        }

        if let Some(offset) = self.pending_bits_offset() {
            // Pending bits are writable
            set_dword(writable_bits, offset, PENDING_BITS_MASK).unwrap();
        }
    }
}

// FIXME: refactor the whole MSI
pub struct PciMsi {
    pub cap: PciMsiCapability,
    pub offset: PciCapOffset,
}

impl PciMsiMessageSource for PciMsi {
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        let _ = config;

        self.cap.is_enabled()
    }

    fn is_masked(&mut self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        let _ = config;
        let _ = vector;

        // TODO
        false
    }

    fn set_pending_bit(
        &mut self,
        config: &mut PciConfigurationSpace,
        vector: PciMsiVector,
        pending: bool,
    ) -> Result<()> {
        let _ = config;
        let _ = vector;
        let _ = pending;

        // TODO
        todo!()
    }

    fn get_message_for(&mut self, vector: PciMsiVector) -> Result<PciMsiMessage> {
        self.cap.get_message_for(vector as u8)
    }

    fn is_valid_vector(&mut self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        let _ = config;
        self.cap.is_valid_vector(vector as u8)
    }

    fn is_pending(&mut self, vector: PciMsiVector) -> bool {
        let _ = vector;
        todo!()
    }
}

impl PciConfigurationSpaceIoHandler for PciMsi {
    type Context<'a> = ();
    type R = PciInterruptConfigEvent;

    fn postprocess_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        offset: usize,
        size: usize,
        _context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let mut data = [0xFFu8; 4];
        config.read(offset, &mut data[..size]);

        let Some(offset_within_cap) = offset_within_range(offset, self.offset, self.cap.size())
        else {
            // Outside
            return Ok(PciHandlerResult::Unhandled);
        };

        let Some((start, end)) = access_data_window(offset, size, self.offset, self.cap.size())
        else {
            // Outside
            return Ok(PciHandlerResult::Unhandled);
        };

        // FIXME: Return proper event when config data was changed
        self.cap.write(offset_within_cap, &data[start..end])?;

        Ok(PciHandlerResult::Handled(PciInterruptConfigEvent::Other))
    }

    fn preprocess_read_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        offset: usize,
        size: usize,
        _context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let Some(offset_within_cap) = offset_within_range(offset, self.offset, self.cap.size())
        else {
            return Ok(PciHandlerResult::Unhandled);
        };

        let Some((start, end)) = access_data_window(offset, size, self.offset, self.cap.size())
        else {
            return Ok(PciHandlerResult::Unhandled);
        };

        // FIXME: Instead of read-modify-write, use config as register storage
        let mut data = [0xFFu8; 4];
        config.read(offset, &mut data[..size]);
        self.cap.read(offset_within_cap, &mut data[start..end])?;
        config.set(offset, &data[..size]);

        Ok(PciHandlerResult::Handled(PciInterruptConfigEvent::Other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiple_message_from() {
        assert_eq!(
            MsiMultipleMessage::try_from(0b0),
            Ok(MsiMultipleMessage::One)
        );

        assert_eq!(
            MsiMultipleMessage::try_from(0b101),
            Ok(MsiMultipleMessage::ThirtyTwo)
        );

        assert!(MsiMultipleMessage::try_from(0b110).is_err());
    }

    #[test]
    fn test_cap_size() {
        // 32bit
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Disabled,
        );
        assert_eq!(cap.size(), 8);

        // 64bit
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Disabled,
        );
        assert_eq!(cap.size(), 12);

        // 32bit + pending
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.size(), 18);

        // 64bit + pending
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.size(), 22);
    }

    #[test]
    fn test_capable_vectors() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.capable_vectors(), MsiMultipleMessage::One);

        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.capable_vectors(), MsiMultipleMessage::ThirtyTwo);
    }

    #[test]
    fn test_num_capable_vectors() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.num_capable_vectors(), 1);

        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.num_capable_vectors(), 32);
    }

    #[test]
    fn test_enabled_vectors() {
        let mut cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.enabled_vectors(), MsiMultipleMessage::One);

        cap.registers[MESSAGE_CONTROL] |= 0b101 << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT;
        assert_eq!(cap.enabled_vectors(), MsiMultipleMessage::ThirtyTwo);
    }

    #[test]
    fn test_num_enabled_vectors() {
        let mut cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        assert_eq!(cap.num_enabled_vectors(), 1);

        cap.registers[MESSAGE_CONTROL] |= 0b101 << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT;
        assert_eq!(cap.num_enabled_vectors(), 32);
    }

    #[test]
    fn test_get_message_for_invalid_vector() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Disabled,
        );

        assert!(cap.get_message_for(2).is_err())
    }

    const TEST_ADDRESS_32: u32 = 0xBAADCAFE;
    const TEST_ADDRESS_64: u64 = 0x12345678_BAADCAFE;
    const TEST_DATA: u32 = 0x1EE7;

    #[test]
    fn test_get_message_for() {
        let mut cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Disabled,
        );
        cap.registers[MESSAGE_ADDRESS..MESSAGE_ADDRESS + 4]
            .copy_from_slice(&TEST_ADDRESS_32.to_ne_bytes());
        cap.registers[MESSAGE_DATA_32BIT..MESSAGE_DATA_32BIT + 2]
            .copy_from_slice(&TEST_DATA.to_ne_bytes()[..2]);
        let msi = cap.get_message_for(0).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: TEST_ADDRESS_32 as u64,
                data: TEST_DATA,
            }
        );

        // set enabled vectors
        let vectors_en = 0b101;
        cap.registers[MESSAGE_CONTROL] |= vectors_en << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT;

        // Multiple messages enabled field determines how many bits software is allowed to modify:
        // 32 vectors enabled -> 0b101 -> 5 bits lowest bits are allowed to be modified
        let vector = (1 << vectors_en) - 1;
        let msi = cap.get_message_for(vector).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: TEST_ADDRESS_32 as u64,
                data: (TEST_DATA & !(vectors_en as u32)) | vector as u32,
            }
        );
    }

    #[test]
    fn test_get_message_for_64bit() {
        let mut cap = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Disabled,
        );
        cap.registers[MESSAGE_ADDRESS..MESSAGE_ADDRESS + 8]
            .copy_from_slice(&TEST_ADDRESS_64.to_ne_bytes());
        cap.registers[MESSAGE_DATA_64BIT..MESSAGE_DATA_64BIT + 2]
            .copy_from_slice(&TEST_DATA.to_ne_bytes()[..2]);
        let msi = cap.get_message_for(0).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: TEST_ADDRESS_64,
                data: TEST_DATA,
            }
        );

        // set enabled vectors
        let vectors_en = 0b101;
        cap.registers[MESSAGE_CONTROL] |= vectors_en << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT;

        // Multiple messages enabled field determines how many bits software is allowed to modify:
        // 32 vectors enabled -> 0b101 -> 5 bits lowest bits are allowed to be modified
        let vector = (1 << vectors_en) - 1;
        let msi = cap.get_message_for(vector).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: TEST_ADDRESS_64,
                data: (TEST_DATA & !(vectors_en as u32)) | vector as u32,
            }
        );
    }

    #[test]
    fn test_send_msi() {
        todo!()
    }

    #[test]
    fn test_set_pending_bit() {
        todo!()
    }

    #[test]
    fn test_clear_pending_bit() {
        todo!()
    }

    #[test]
    fn test_is_masked() {
        todo!()
    }
}
