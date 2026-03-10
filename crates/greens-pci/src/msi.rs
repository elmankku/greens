// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::marker::PhantomData;
use std::mem;

use crate::PciInterruptController;
use crate::capability::{PciCapOffset, PciCapability, PciCapabilityId};
use crate::config_handler::PciConfigurationSpaceIoHandler;
use crate::configuration_space::{PciConfigurationSpace, is_bus_master};
use crate::function::{PciConfigurationUpdate, PciHandlerResult};
use crate::utils::range_overlaps;
use crate::utils::register_block::{set_dword, set_word};
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
    fn is_valid_vector(&self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool;

    /// Checks if the MSI message source is enabled.
    ///
    /// Returns `true` when the source is enabled. Otherwise returns `false`.
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool;

    /// Checks if `vector` is masked.
    ///
    /// Returns `true` when the `vector` or the whole function is masked. Otherwise returns `false`.
    fn is_masked(&self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool;

    /// Checks if `vector` is pending.
    ///
    /// Returns `true` when the `vector` is pending. Otherwise returns `false`
    fn is_pending(&self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool;

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
    fn get_message_for(
        &mut self,
        config: &PciConfigurationSpace,
        vector: PciMsiVector,
    ) -> Result<PciMsiMessage>;

    /// Attempts to generate an MSI message for `vector`.
    ///
    /// Checks all conditions (enabled bit, vector and function masks, etc.) and if an MSI
    /// generation is successful, returns `Ok(PciMsiGenerationResult::Generated(PciMessage))`
    /// with configured address and data. If the vector is masked, sets the pending bit and
    /// returns `Ok(Masked)`. If a message cannot be generated, returns an `Err` with appropriate
    /// `Error` variant.
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
            return Err(Error::MsiDisabled);
        }

        if self.is_masked(config, vector) {
            self.set_pending_bit(config, vector, true)?;
            return Ok(PciMsiGenerationResult::Masked);
        }

        self.set_pending_bit(config, vector, false)?;
        let message = self.get_message_for(config, vector)?;

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

    pub fn max_vectors(&self) -> u16 {
        1 << (*self as u16)
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

pub struct PciMsiCapability {
    address_width: MsiAddressWidth,
    per_vector_masking: MsiPerVectorMasking,
    vectors: MsiMultipleMessage,
}

impl PciMsiCapability {
    pub fn new(
        address_width: MsiAddressWidth,
        vectors: MsiMultipleMessage,
        per_vector_masking: MsiPerVectorMasking,
    ) -> Self {
        Self {
            address_width,
            per_vector_masking,
            vectors,
        }
    }

    fn to_message_control(&self) -> u16 {
        self.address_width as u16 | self.vectors.to_capable() | self.per_vector_masking as u16
    }
}

impl PciCapability for PciMsiCapability {
    fn id(&self) -> PciCapabilityId {
        PciCapabilityId::Msi
    }

    fn size(&self) -> usize {
        cap_size(self.to_message_control())
    }

    fn registers(&self, registers: &mut [u8]) {
        registers.fill(0);
        set_word(registers, MESSAGE_CONTROL, self.to_message_control());
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        let control = self.to_message_control();
        // Control register: multiple messages enable and MSI enable
        set_word(
            writable_bits,
            MESSAGE_CONTROL,
            MESSAGE_CONTROL_MULTIPLE_MSG_EN | MESSAGE_CONTROL_MSI_EN,
        );

        // Message address: bits [64:02] are writable
        set_dword(writable_bits, MESSAGE_ADDRESS, MESSAGE_ADDRESS_MASK);
        if is_64bit(control) {
            set_dword(
                writable_bits,
                MESSAGE_ADDRESS_UPPER,
                MESSAGE_ADDRESS_UPPER_MASK,
            );
        }

        // Message data is writable
        set_word(
            writable_bits,
            message_data_offset(control),
            MESSAGE_DATA_MASK,
        );

        if let Some(offset) = mask_bits_offset(control) {
            // Mask bits are writable
            set_dword(writable_bits, offset, MASK_BITS_MASK);
        }

        if let Some(offset) = pending_bits_offset(control) {
            // Pending bits are writable
            set_dword(writable_bits, offset, PENDING_BITS_MASK);
        }
    }
}

fn is_enabled(control: u16) -> bool {
    control & MESSAGE_CONTROL_MSI_EN == MESSAGE_CONTROL_MSI_EN
}

fn cap_size(control: u16) -> usize {
    // 2 (control) + 4 (message address) + 2 (message data)
    let mut size = 8;

    if per_vector_masking_enabled(control) {
        // 2 (reserved) + 4 (mask bits) + 4 (pending bits)
        size += 10;
    }

    if is_64bit(control) {
        // 4 (upper message address)
        size += 4;
    }

    size
}

fn is_64bit(control: u16) -> bool {
    let mask = MsiAddressWidth::Address64Bit as u16;
    control & mask == mask
}

fn per_vector_masking_enabled(control: u16) -> bool {
    let mask = MsiPerVectorMasking::Enabled as u16;
    control & mask == mask
}

fn message_data_offset(control: u16) -> usize {
    match is_64bit(control) {
        true => MESSAGE_DATA_64BIT,
        false => MESSAGE_DATA_32BIT,
    }
}

fn mask_bits_offset(control: u16) -> Option<usize> {
    if per_vector_masking_enabled(control) {
        let offset = match is_64bit(control) {
            true => MASK_BITS_64BIT,
            false => MASK_BITS_32BIT,
        };
        return Some(offset);
    }
    None
}

fn pending_bits_offset(control: u16) -> Option<usize> {
    if per_vector_masking_enabled(control) {
        let offset = match is_64bit(control) {
            true => PENDING_BITS_64BIT,
            false => PENDING_BITS_32BIT,
        };
        return Some(offset);
    }
    None
}

fn capable_vectors(control: u16) -> MsiMultipleMessage {
    MsiMultipleMessage::from_capable(control).unwrap_or(MsiMultipleMessage::One)
}

fn num_capable_vectors(control: u16) -> u16 {
    capable_vectors(control).max_vectors()
}

fn enabled_vectors(control: u16) -> MsiMultipleMessage {
    MsiMultipleMessage::from_enable(control).unwrap_or(MsiMultipleMessage::One)
}

pub struct PciMsi<T: PciInterruptController> {
    pub offset: PciCapOffset,
    phantom: PhantomData<T>,
}

impl<T> PciMsi<T>
where
    T: PciInterruptController,
{
    pub fn new(offset: PciCapOffset) -> Self {
        Self {
            offset,
            phantom: PhantomData,
        }
    }

    fn get_message_data_for(&self, config: &PciConfigurationSpace, vector: usize) -> Option<u16> {
        if !self.is_valid_vector(config, vector) {
            return None;
        }

        let control = config.read_word(self.offset);
        let message_data = config.read_word(self.offset + message_data_offset(control));

        // Bit combination of enabled vectors determines the number of bits the function is allowed
        // to modify, f.ex. 0b010 -> 2bits. Unfortunately the specification does not specify how,
        // although it is implied that the modifiable bits are set to 0 by the system software.
        //
        // For simplicity, just replace the base data bits and insert the vector.
        let modifiable_mask = (1 << enabled_vectors(control) as u16) - 1;

        Some((message_data & !modifiable_mask) | vector as u16)
    }

    fn should_handle_access(
        &self,
        config: &PciConfigurationSpace,
        offset: usize,
        size: usize,
    ) -> bool {
        range_overlaps(
            offset,
            size,
            self.offset,
            cap_size(config.read_word(self.offset)),
        )
    }
}

impl<T> PciMsiMessageSource for PciMsi<T>
where
    T: PciInterruptController,
{
    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        let _ = config;
        is_enabled(config.read_word(self.offset))
    }

    fn is_masked(&self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        let Some(offset) = mask_bits_offset(config.read_word(self.offset)) else {
            return false;
        };

        config.read_dword(self.offset + offset) & (1 << vector) != 0
    }

    fn set_pending_bit(
        &mut self,
        config: &mut PciConfigurationSpace,
        vector: PciMsiVector,
        pending: bool,
    ) -> Result<()> {
        let Some(offset) = pending_bits_offset(config.read_word(self.offset)) else {
            // Vector masking is not supported by this cap.
            return Err(Error::NotSupported);
        };

        if pending && !self.is_masked(config, vector) {
            // Setting vector pending is only allowed when vector is masked.
            return Err(Error::VectorNotMasked { vector });
        }

        let offset = self.offset + offset;

        // Update pending state for vector.
        let mut bits = config.read_dword(offset);
        match pending {
            true => bits |= 1 << vector,
            false => bits &= !(1 << vector),
        };
        config.write_dword(offset, bits);

        Ok(())
    }

    fn get_message_for(
        &mut self,
        config: &PciConfigurationSpace,
        vector: PciMsiVector,
    ) -> Result<PciMsiMessage> {
        let control = config.read_word(self.offset);

        let mut address: u64 = config.read_dword(self.offset + MESSAGE_ADDRESS) as u64;
        if is_64bit(control) {
            address |= (config.read_dword(self.offset + MESSAGE_ADDRESS_UPPER) as u64) << 32;
        }

        let Some(data) = self.get_message_data_for(config, vector) else {
            return Err(Error::InvalidMsiVector {
                vector: vector as u8,
            });
        };

        Ok(PciMsiMessage {
            address,
            data: data as u32,
        })
    }

    fn is_valid_vector(&self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        // The Multiple Message Capable field determines the number of valid vectors. However, the
        // software may enable less than what the device is capable of. That is handled in message
        // generation: the Multiple Message Enable controls how many LSB bits in MSI data the device
        // is allowed to modify.
        vector < num_capable_vectors(config.read_word(self.offset)) as usize
    }

    fn is_pending(&self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        let Some(offset) = pending_bits_offset(config.read_word(self.offset)) else {
            // Vector masking is not supported by this cap.
            return false;
        };

        config.read_dword(self.offset + offset) & (1 << vector) != 0
    }
}

impl<T> PciConfigurationSpaceIoHandler for PciMsi<T>
where
    T: PciInterruptController,
{
    type Context<'a> = T;
    type R = Option<PciConfigurationUpdate>;

    fn postprocess_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        offset: usize,
        size: usize,
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        if !self.should_handle_access(config, offset, size) {
            return Ok(PciHandlerResult::Unhandled);
        }

        let mut evaluate_interrupts = false;
        let mut control = config.read_word(self.offset);

        // Control register
        if range_overlaps(offset, size, self.offset, 2) {
            // Handle multiple messages
            if let Some(vectors_en) = MsiMultipleMessage::from_enable(control) {
                if vectors_en as u16 > capable_vectors(control) as u16 {
                    // Invalid multiple messages, default to 0
                    control &= !MESSAGE_CONTROL_MULTIPLE_MSG_EN;
                    config.write_word(self.offset, control);
                }
            } else {
                // Illegal multiple messages, default to 0
                control &= !MESSAGE_CONTROL_MULTIPLE_MSG_EN;
                config.write_word(self.offset, control);
            }

            let mask = MESSAGE_CONTROL_MSI_EN | MESSAGE_CONTROL_VECTOR_MASKING;
            if control & mask == mask {
                // Possibly enabled, evaluate pending interrupts
                evaluate_interrupts = true;
            }
        }

        // Vector masking
        if mask_bits_offset(control).is_some_and(|masking_offset| {
            range_overlaps(offset, size, self.offset + masking_offset, 4)
        }) {
            evaluate_interrupts = true;
        }

        if evaluate_interrupts {
            evaluate_pending_interrupts(config, self, context);
        }

        // Message address
        let address_width = if is_64bit(control) { 8 } else { 4 };
        if range_overlaps(offset, size, self.offset + MESSAGE_ADDRESS, address_width)
            || range_overlaps(offset, size, self.offset + message_data_offset(control), 2)
        {
            let msi = self.get_message_for(config, 0)?;
            return Ok(PciHandlerResult::Handled(Some(
                PciConfigurationUpdate::MsiMessage(msi),
            )));
        }

        Ok(PciHandlerResult::Handled(None))
    }

    fn preprocess_read_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        offset: usize,
        size: usize,
        _context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        match self.should_handle_access(config, offset, size) {
            true => Ok(PciHandlerResult::Handled(None)),
            false => Ok(PciHandlerResult::Unhandled),
        }
    }
}

fn next_pending_vector<T: PciInterruptController>(
    config: &PciConfigurationSpace,
    msi: &PciMsi<T>,
    since: PciMsiVector,
) -> Option<PciMsiVector> {
    let control = config.read_word(msi.offset);
    let offset = pending_bits_offset(control)?;

    let mut pending = config.read_dword(msi.offset + offset);
    pending &= !1u32
        .checked_shl(since as u32)
        .unwrap_or(0)
        .checked_sub(1)
        .unwrap_or(u32::MAX);

    let vector = pending.trailing_zeros() as u16;

    // According to the spec, the Multiple Message Capable indicates how many vectors with mask and
    // pending bits are implemented. The rest are reserved.
    if vector < num_capable_vectors(control) {
        Some(vector as usize)
    } else {
        None
    }
}

fn evaluate_pending_interrupts<T: PciInterruptController>(
    config: &mut PciConfigurationSpace,
    msi: &mut PciMsi<T>,
    interrupt_controller: &mut T,
) {
    let mut start = 0;
    while let Some(vector) = next_pending_vector(config, msi, start) {
        if let Ok(PciMsiGenerationResult::Generated(message)) =
            msi.try_generate_message(config, vector)
        {
            interrupt_controller.send_msi(message)
        }

        start = vector + 1;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::configuration_space::PciConfigurationSpace;
    use crate::registers::{PCI_COMMAND, PCI_COMMAND_BUS_MASTER_MASK};

    use super::*;

    // IRL the driver controls bus master bit.
    pub fn set_bus_master(config: &mut PciConfigurationSpace, enable: bool) {
        let mut command = config.read_word(PCI_COMMAND);

        if enable {
            command |= PCI_COMMAND_BUS_MASTER_MASK;
        } else {
            command &= !PCI_COMMAND_BUS_MASTER_MASK;
        }

        config.set_word(PCI_COMMAND, command);
    }

    #[derive(Debug, Default)]
    pub struct TestIrqController {
        pub messages: Vec<PciMsiMessage>,
    }

    impl PciInterruptController for TestIrqController {
        fn set_interrupt(
            &mut self,
            _line: crate::intx::PciInterruptLine,
            _state: crate::intx::PciInterruptLineState,
        ) {
            unreachable!()
        }

        fn send_msi(&mut self, message: PciMsiMessage) {
            self.messages.push(message);
        }
    }

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

    fn new_config(cap: &PciMsiCapability) -> (PciConfigurationSpace, PciCapOffset) {
        let mut config = PciConfigurationSpace::new();
        let offset = config.add_capability(cap).expect("adding cap failed");

        (config, offset)
    }

    #[test]
    fn test_capable_vectors() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        let (config, offset) = new_config(&cap);

        assert_eq!(capable_vectors(config.read_word(offset)), cap.vectors);

        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        let (config, offset) = new_config(&cap);

        assert_eq!(capable_vectors(config.read_word(offset)), cap.vectors);
    }

    #[test]
    fn test_num_capable_vectors() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        let (config, offset) = new_config(&cap);

        assert_eq!(num_capable_vectors(config.read_word(offset)), 1);

        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        let (config, offset) = new_config(&cap);

        assert_eq!(num_capable_vectors(config.read_word(offset)), 32);
    }

    #[test]
    fn test_message_control_ro_fields() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Disabled,
        );
        let (mut config, offset) = new_config(&cap);

        let expect = config.read_word(offset);
        // bits 15:9 are reserved and RAZ
        let mask = 0b1111_1110_0000_0000u16;
        assert_eq!(expect & mask, 0);

        config.write_word(offset, expect | mask);
        config.read_word(offset);
        assert_eq!(config.read_word(offset), expect);

        // Per-vector masking is RO
        assert_eq!(expect & MESSAGE_CONTROL_VECTOR_MASKING, 0);

        config.write_word(offset, expect | MESSAGE_CONTROL_VECTOR_MASKING);
        assert_eq!(config.read_word(offset), expect);

        // 64bit address capable is RO
        assert_eq!(expect & MESSAGE_CONTROL_64BIT, 0);

        config.write_word(offset, expect | MESSAGE_CONTROL_64BIT);
        assert_eq!(config.read_word(offset), expect);

        // Capable vectors is RO
        assert_eq!(expect & MESSAGE_CONTROL_MULTIPLE_MSG_CAP, 0);

        config.write_word(offset, expect | MESSAGE_CONTROL_MULTIPLE_MSG_CAP);
        assert_eq!(config.read_word(offset), expect);
    }

    #[test]
    fn test_enabled_vectors() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, offset) = new_config(&cap);

        let control = config.read_word(offset);
        assert_eq!(enabled_vectors(control), MsiMultipleMessage::One);

        config.write_word(offset, control | MsiMultipleMessage::ThirtyTwo.to_enable());

        let control = config.read_word(offset);
        assert_eq!(enabled_vectors(control), MsiMultipleMessage::ThirtyTwo);
    }

    fn new_cap<T: PciInterruptController>(
        cap: &PciMsiCapability,
    ) -> (PciConfigurationSpace, PciMsi<T>) {
        let (config, offset) = new_config(cap);
        (config, PciMsi::new(offset))
    }

    #[test]
    fn test_get_message_for_invalid_vector() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::Two,
            MsiPerVectorMasking::Disabled,
        );
        let (config, mut cap) = new_cap::<TestIrqController>(&cap);

        assert!(cap.get_message_for(&config, 1).is_ok());
        assert!(cap.get_message_for(&config, 2).is_err());
    }

    const TEST_ADDRESS_32: u32 = 0xBAADCAFE;
    const TEST_ADDRESS_64: u64 = 0x12345678_BAADCAFE;
    const TEST_DATA: u32 = 0x1EE7;

    // Splits 64bit address into 32bit parts.
    fn address_to_components(address: u64) -> (u32, u32) {
        let lower = address as u32;
        let upper = (address >> 32) as u32;

        (upper, lower)
    }

    // Creates a valid msi address from address components
    fn msi_address(upper: Option<u32>, lower: Option<u32>) -> u64 {
        let upper = upper.unwrap_or(0) & MESSAGE_ADDRESS_UPPER_MASK;
        let lower = lower.unwrap_or(0) & MESSAGE_ADDRESS_MASK;

        (upper as u64) << 32 | lower as u64
    }

    // Creates a valid msi data for `vector` based on number of enabled vectors and data
    fn msi_data(vector: usize, control: u16, data: u32) -> u32 {
        assert!(vector < 32);

        // Multiple messages enabled field determines how many bits software is allowed to modify:
        // 32 vectors enabled -> multiple message = 0b101 -> 5 bits lowest bits are allowed to be
        // modified. Therefore, low bits for vector 31 is 0b1_1111.
        let enabled_vectors = MsiMultipleMessage::from_enable(control).expect("invalid control");
        let modifiable_bits = (enabled_vectors.max_vectors() - 1) as u32;

        (data & !modifiable_bits) | vector as u32
    }

    fn init_32bit_test_data<T: PciInterruptController>(
        config: &mut PciConfigurationSpace,
        cap: &PciMsi<T>,
    ) {
        config.write_dword(cap.offset + MESSAGE_ADDRESS, TEST_ADDRESS_32);
        config.write_word(cap.offset + MESSAGE_DATA_32BIT, TEST_DATA as u16);

        // Enable
        config.write_word(
            cap.offset,
            config.read_word(cap.offset) | MESSAGE_CONTROL_MSI_EN,
        );
    }

    fn init_64bit_test_data<T: PciInterruptController>(
        config: &mut PciConfigurationSpace,
        cap: &PciMsi<T>,
    ) -> (u32, u32) {
        let (upper, lower) = address_to_components(TEST_ADDRESS_64);
        config.write_dword(cap.offset + MESSAGE_ADDRESS, lower);
        config.write_dword(cap.offset + MESSAGE_ADDRESS_UPPER, upper);
        config.write_word(cap.offset + MESSAGE_DATA_64BIT, TEST_DATA as u16);

        // Enable
        config.write_word(
            cap.offset,
            config.read_word(cap.offset) | MESSAGE_CONTROL_MSI_EN,
        );

        (upper, lower)
    }

    fn enable_vectors<T: PciInterruptController>(
        config: &mut PciConfigurationSpace,
        cap: &PciMsi<T>,
        vectors: &MsiMultipleMessage,
    ) {
        let control = config.read_word(cap.offset) | vectors.to_enable();
        config.write_word(cap.offset, control);
    }

    #[test]
    fn test_get_message_for_32bit() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Disabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&cap);
        init_32bit_test_data(&mut config, &cap);

        let msi = cap.get_message_for(&config, 0).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: msi_address(None, Some(TEST_ADDRESS_32)),
                data: msi_data(0, config.read_word(cap.offset), TEST_DATA),
            }
        );

        let vector = 31;
        enable_vectors(&mut config, &cap, &MsiMultipleMessage::ThirtyTwo);

        let msi = cap.get_message_for(&config, vector).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: msi_address(None, Some(TEST_ADDRESS_32)),
                data: msi_data(vector, config.read_word(cap.offset), TEST_DATA),
            }
        );

        // The software is allowed to enabled less than the device is capable of: the number of
        // bits that the device is allowed to modify from MSI data changes.
        let vector = 31;
        enable_vectors(&mut config, &cap, &MsiMultipleMessage::Sixteen);

        let msi = cap.get_message_for(&config, vector).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: msi_address(None, Some(TEST_ADDRESS_32)),
                data: msi_data(vector, config.read_word(cap.offset), TEST_DATA),
            }
        );
    }

    #[test]
    fn test_get_message_for_64bit() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Disabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&cap);
        let (upper, lower) = init_64bit_test_data(&mut config, &cap);

        let vector = 0;
        let msi = cap.get_message_for(&config, vector).unwrap();

        assert_eq!(
            msi,
            PciMsiMessage {
                address: msi_address(Some(upper), Some(lower)),
                data: msi_data(vector, config.read_word(cap.offset), TEST_DATA)
            }
        );

        let vector = 31;
        enable_vectors(&mut config, &cap, &MsiMultipleMessage::ThirtyTwo);
        let msi = cap.get_message_for(&config, vector).unwrap();

        assert_eq!(
            msi,
            PciMsiMessage {
                address: msi_address(Some(upper), Some(lower)),
                data: msi_data(vector, config.read_word(cap.offset), TEST_DATA),
            }
        );

        // The software is allowed to enabled less than the device is capable of: the number of
        // bits that the device is allowed to modify from MSI data changes.
        let vector = 31;
        enable_vectors(&mut config, &cap, &MsiMultipleMessage::Sixteen);

        let msi = cap.get_message_for(&config, vector).unwrap();
        assert_eq!(
            msi,
            PciMsiMessage {
                address: msi_address(Some(upper), Some(lower)),
                data: msi_data(vector, config.read_word(cap.offset), TEST_DATA),
            }
        );
    }

    fn test_masked_vectors<T: PciInterruptController>(
        config: &mut PciConfigurationSpace,
        cap: &PciMsi<T>,
        offset: usize,
    ) {
        for vector in 0..32 {
            assert!(
                !cap.is_masked(&config, vector),
                "{vector} should be unmasked"
            );
            config.write_dword(offset, config.read_dword(offset) | 1 << vector);
            assert!(cap.is_masked(&config, vector), "{vector} should be masked");
        }
    }

    #[test]
    fn test_is_masked_32bit() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, cap) = new_cap::<TestIrqController>(&cap);
        init_32bit_test_data(&mut config, &cap);

        let offset = cap.offset + MASK_BITS_32BIT;
        test_masked_vectors(&mut config, &cap, offset);
    }

    #[test]
    fn test_is_masked_64bit() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, cap) = new_cap::<TestIrqController>(&cap);
        init_64bit_test_data(&mut config, &cap);

        let offset = cap.offset + MASK_BITS_64BIT;
        test_masked_vectors(&mut config, &cap, offset);
    }

    fn test_pending_bits<T: PciInterruptController>(
        config: &mut PciConfigurationSpace,
        cap: &mut PciMsi<T>,
        offset: usize,
    ) {
        for vector in 0..32 {
            assert!(
                !cap.is_pending(&config, vector),
                "{vector} should not be pending"
            );

            // Set pending bit only allowed for masked vectors.
            assert!(cap.set_pending_bit(config, vector, true).is_err());

            // First mask vector...
            config.write_dword(offset - 4, 1 << vector);

            // Then test set pending.
            cap.set_pending_bit(config, vector, true)
                .expect("set pending failed");
            assert!(
                cap.is_pending(&config, vector),
                "{vector} should be pending"
            );
            assert_eq!(
                config.read_dword(offset),
                1 << vector,
                "unexpected pending bit mask"
            );

            // Test clear pending.
            cap.set_pending_bit(config, vector, false)
                .expect("clear pending failed");
            assert!(
                !cap.is_pending(&config, vector),
                "{vector} should not be pending"
            );
            assert_eq!(config.read_dword(offset), 0);
        }
    }

    #[test]
    fn test_pending_bits_32bit() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );

        let (mut config, mut cap) = new_cap::<TestIrqController>(&cap);
        let offset = cap.offset + PENDING_BITS_32BIT;
        test_pending_bits(&mut config, &mut cap, offset);
    }

    #[test]
    fn test_pending_bits_64bit() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );

        let (mut config, mut cap) = new_cap::<TestIrqController>(&cap);
        let offset = cap.offset + PENDING_BITS_64BIT;
        test_pending_bits(&mut config, &mut cap, offset);
    }

    #[test]
    fn try_generate_message() {
        let cap = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::Two,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&cap);

        // Is not a bus master.
        assert_eq!(
            cap.try_generate_message(&mut config, 0),
            Err(Error::NotBusMaster)
        );

        set_bus_master(&mut config, true);

        // Is not enabled.
        assert_eq!(
            cap.try_generate_message(&mut config, 0),
            Err(Error::MsiDisabled)
        );

        init_32bit_test_data(&mut config, &cap);

        // Invalid vector.
        assert_eq!(
            cap.try_generate_message(&mut config, 3),
            Err(Error::InvalidMsiVector { vector: 3 })
        );

        // Vector masked.
        config.write_dword(cap.offset + MASK_BITS_32BIT, 1);
        assert_eq!(
            cap.try_generate_message(&mut config, 0),
            Ok(PciMsiGenerationResult::Masked)
        );
        assert!(cap.is_pending(&config, 0));

        // Clear mask.
        config.write_dword(cap.offset + MASK_BITS_32BIT, 0);

        // Generates message.
        let res = cap
            .try_generate_message(&mut config, 0)
            .expect("generate message");
        assert_eq!(
            res,
            PciMsiGenerationResult::Generated(PciMsiMessage {
                // Handles address alignment.
                address: msi_address(None, Some(TEST_ADDRESS_32)),
                // Sets vector information to low bits.
                data: msi_data(0, config.read_word(cap.offset), TEST_DATA),
            })
        );

        // Generation clears pending bit.
        assert!(!cap.is_pending(&config, 0));
    }

    #[test]
    fn test_preprocess_read_config() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();

        // Test out of bounds.
        assert_eq!(
            cap.preprocess_read_config(&mut config, cap.offset - 1, 1, &mut ic),
            Ok(PciHandlerResult::Unhandled)
        );
        assert_eq!(
            cap.preprocess_read_config(
                &mut config,
                cap.offset + cap_size(c.to_message_control()),
                1,
                &mut ic
            ),
            Ok(PciHandlerResult::Unhandled)
        );

        // Test within bounds.
        assert_eq!(
            cap.preprocess_read_config(&mut config, cap.offset, 1, &mut ic),
            Ok(PciHandlerResult::Handled(None))
        );
        assert_eq!(
            cap.preprocess_read_config(
                &mut config,
                cap.offset + cap_size(c.to_message_control()) - 1,
                1,
                &mut ic
            ),
            Ok(PciHandlerResult::Handled(None))
        );
    }

    #[test]
    fn test_postprocess_write_config_bounds() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();

        // Test out of bounds.
        assert_eq!(
            cap.postprocess_write_config(&mut config, cap.offset - 1, 1, &mut ic),
            Ok(PciHandlerResult::Unhandled)
        );
        assert_eq!(
            cap.postprocess_write_config(
                &mut config,
                cap.offset + cap_size(c.to_message_control()),
                1,
                &mut ic
            ),
            Ok(PciHandlerResult::Unhandled)
        );
    }

    #[test]
    fn test_postprocess_write_config_control_ro_bits() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Disabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();

        // Write R/O fields reports handled, but no changes to data.
        config.write_word(
            cap.offset,
            !(MESSAGE_CONTROL_MSI_EN | MESSAGE_CONTROL_MULTIPLE_MSG_EN),
        );
        assert_eq!(
            cap.postprocess_write_config(&mut config, cap.offset, 2, &mut ic),
            Ok(PciHandlerResult::Handled(None))
        );
        assert_eq!(config.read_word(cap.offset), c.to_message_control());
    }

    fn test_write_multiple_message<T: PciInterruptController>(
        config: &mut PciConfigurationSpace,
        cap: &mut PciMsi<T>,
        ic: &mut T,
    ) {
        let control = config.read_word(cap.offset);
        let capable_vectors = MsiMultipleMessage::from_capable(config.read_word(cap.offset))
            .expect("invalid capable vectors");

        // Test multiple message within allowed range.
        for vectors in 0..capable_vectors as u16 {
            let expect = control | vectors << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT;
            config.write_word(cap.offset, expect);

            assert_eq!(
                cap.postprocess_write_config(config, cap.offset, 2, ic),
                Ok(PciHandlerResult::Handled(None))
            );
            assert_eq!(config.read_word(cap.offset), expect, "vectors: {vectors}");
        }

        // Test multiple message outside allowed range (capable + 1).
        config.write_word(
            cap.offset,
            control | ((capable_vectors as u16 + 1) << MESSAGE_CONTROL_MULTIPLE_MSG_EN_SHIFT),
        );
        assert_eq!(
            cap.postprocess_write_config(config, cap.offset, 2, ic),
            Ok(PciHandlerResult::Handled(None))
        );

        // Invalid value defaults to 0.
        assert_eq!(
            config.read_word(cap.offset),
            control & !MESSAGE_CONTROL_MULTIPLE_MSG_EN
        );
    }

    #[test]
    fn test_postprocess_write_config_multiple_message() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::One,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();

        test_write_multiple_message(&mut config, &mut cap, &mut ic);

        let c = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::ThirtyTwo,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();

        test_write_multiple_message(&mut config, &mut cap, &mut ic);
    }

    fn set_vector_mask<T: PciInterruptController>(
        config: &mut PciConfigurationSpace,
        cap: &PciMsi<T>,
        vector: PciMsiVector,
        mask: bool,
    ) {
        let offset =
            cap.offset + mask_bits_offset(config.read_word(cap.offset)).expect("mask offset");

        let bits = config.read_dword(offset);
        let bits = match mask {
            true => bits | 1 << vector,
            false => bits & !(1 << vector),
        };

        config.write_dword(offset, bits);
    }

    #[test]
    fn test_postprocess_write_config_enable() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::Eight,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();
        set_bus_master(&mut config, true);

        // Set vector 2 pending.
        set_vector_mask(&mut config, &cap, 2, true);
        cap.set_pending_bit(&mut config, 2, true)
            .expect("set pending");
        set_vector_mask(&mut config, &cap, 2, false);
        assert!(ic.messages.is_empty());

        // Enable: should evaluate pending interrupts.
        let control = config.read_word(cap.offset);
        config.write_word(cap.offset, control | MESSAGE_CONTROL_MSI_EN);
        assert_eq!(
            cap.postprocess_write_config(&mut config, cap.offset, 2, &mut ic),
            Ok(PciHandlerResult::Handled(None))
        );

        // Pending interrupts evaluated.
        assert_eq!(ic.messages.len(), 1);
    }

    #[test]
    fn test_postprocess_write_config_msi_fields_32bit() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::Eight,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();

        let address = msi_address(None, Some(TEST_ADDRESS_32));
        let lower_offset = cap.offset + MESSAGE_ADDRESS;
        let data_offset = cap.offset + MESSAGE_DATA_32BIT;

        // Write MSI lower address and post-process: reports configuration update.
        let expected = PciConfigurationUpdate::MsiMessage(PciMsiMessage {
            address,
            data: msi_data(0, config.read_word(cap.offset), 0),
        });

        config.write_dword(lower_offset, TEST_ADDRESS_32);
        assert_eq!(
            cap.postprocess_write_config(&mut config, lower_offset, 4, &mut ic),
            Ok(PciHandlerResult::Handled(Some(expected)))
        );

        // Write to MSI data and post-process: reports configuration update.
        let expected = PciConfigurationUpdate::MsiMessage(PciMsiMessage {
            address,
            data: msi_data(0, config.read_word(cap.offset), TEST_DATA),
        });

        config.write_dword(data_offset, TEST_DATA);
        assert_eq!(
            cap.postprocess_write_config(&mut config, data_offset, 2, &mut ic),
            Ok(PciHandlerResult::Handled(Some(expected)))
        );
    }

    #[test]
    fn test_postprocess_write_config_msi_fields_64bit() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::Eight,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();

        let (upper, lower) = address_to_components(TEST_ADDRESS_64);
        let lower_offset = cap.offset + MESSAGE_ADDRESS;
        let upper_offset = cap.offset + MESSAGE_ADDRESS_UPPER;
        let data_offset = cap.offset + MESSAGE_DATA_64BIT;

        // Write MSI lower address and post-process: reports configuration update.
        let expected = PciConfigurationUpdate::MsiMessage(PciMsiMessage {
            address: msi_address(Some(0), Some(lower)),
            data: msi_data(0, config.read_word(cap.offset), 0),
        });

        config.write_dword(lower_offset, lower);
        assert_eq!(
            cap.postprocess_write_config(&mut config, lower_offset, 4, &mut ic),
            Ok(PciHandlerResult::Handled(Some(expected)))
        );

        // Write MSI upper address and post-process: reports configuration update.
        let expected = PciConfigurationUpdate::MsiMessage(PciMsiMessage {
            address: msi_address(Some(upper), Some(lower)),
            data: msi_data(0, config.read_word(cap.offset), 0),
        });

        config.write_dword(upper_offset, upper);
        assert_eq!(
            cap.postprocess_write_config(&mut config, upper_offset, 4, &mut ic),
            Ok(PciHandlerResult::Handled(Some(expected)))
        );

        // Write to MSI data and post-process: reports configuration update.
        let expected = PciConfigurationUpdate::MsiMessage(PciMsiMessage {
            address: msi_address(Some(upper), Some(lower)),
            data: msi_data(0, config.read_word(cap.offset), TEST_DATA),
        });

        config.write_dword(data_offset, TEST_DATA);
        assert_eq!(
            cap.postprocess_write_config(&mut config, data_offset, 2, &mut ic),
            Ok(PciHandlerResult::Handled(Some(expected)))
        );
    }

    #[test]
    fn test_postprocess_write_config_vector_masking_disabled_32bit() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address32Bit,
            MsiMultipleMessage::Eight,
            MsiPerVectorMasking::Disabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();
        let mask_offset = cap.offset + MASK_BITS_32BIT;

        assert_eq!(
            cap.postprocess_write_config(&mut config, mask_offset, 4, &mut ic),
            Ok(PciHandlerResult::Unhandled)
        );
        assert_eq!(ic.messages.len(), 0);
    }

    #[test]
    fn test_postprocess_write_config_vector_masking_disabled_64bit() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::Eight,
            MsiPerVectorMasking::Disabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();
        let mask_offset = cap.offset + MASK_BITS_64BIT;

        assert_eq!(
            cap.postprocess_write_config(&mut config, mask_offset, 4, &mut ic),
            Ok(PciHandlerResult::Unhandled)
        );
        assert_eq!(ic.messages.len(), 0);
    }

    #[test]
    fn test_postprocess_write_config_vector_masking() {
        let c = PciMsiCapability::new(
            MsiAddressWidth::Address64Bit,
            MsiMultipleMessage::Eight,
            MsiPerVectorMasking::Enabled,
        );
        let (mut config, mut cap) = new_cap::<TestIrqController>(&c);
        let mut ic = TestIrqController::default();
        set_bus_master(&mut config, true);

        // Enable.
        let control = config.read_word(cap.offset);
        config.write_word(
            cap.offset,
            control | MESSAGE_CONTROL_MSI_EN | MsiMultipleMessage::Four.to_enable(),
        );

        // Set two vectors pending, unmask just one first.
        set_vector_mask(&mut config, &cap, 1, true);
        set_vector_mask(&mut config, &cap, 2, true);
        cap.set_pending_bit(&mut config, 1, true)
            .expect("set pending 1");
        cap.set_pending_bit(&mut config, 2, true)
            .expect("set pending 2");

        set_vector_mask(&mut config, &cap, 1, false);

        assert!(ic.messages.is_empty());

        // Write to mask register evaluates interrupts.
        let mask_offset =
            cap.offset + mask_bits_offset(config.read_word(cap.offset)).expect("mask offset");
        assert_eq!(
            cap.postprocess_write_config(&mut config, mask_offset, 4, &mut ic),
            Ok(PciHandlerResult::Handled(None))
        );
        assert_eq!(ic.messages.len(), 1);

        // Unmask the other. Should evaluate remaining interrupt.
        set_vector_mask(&mut config, &cap, 2, false);
        assert_eq!(
            cap.postprocess_write_config(&mut config, mask_offset, 4, &mut ic),
            Ok(PciHandlerResult::Handled(None))
        );
        assert_eq!(ic.messages.len(), 2);
    }
}
