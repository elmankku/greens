// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::iter;
use std::mem;
use std::slice;
use std::slice::{Iter, IterMut};

use crate::bar_region::{PciBarRegionInfo, PciBarRegionSet, PciBarRegionSetHandler};
use crate::capability::{PciCapOffset, PciCapability, PciCapabilityId};
use crate::config_handler::PciConfigurationSpaceIoHandler;
use crate::configuration_space::{allow_bus_master, PciConfigurationSpace};
use crate::function::{PciConfigurationUpdate, PciHandlerResult};
use crate::msi::{PciMsiGenerationResult, PciMsiMessageSource, PciMsiVector};
use crate::utils::range_overlaps;
use crate::utils::register_block::{
    set_dword, set_word, CheckedRegisterBlockAutoImpl, CheckedRegisterBlockReader,
    CheckedRegisterBlockSetter, ReadableRegisterBlock, RegisterBlockAccessValidator,
    RegisterBlockAutoImpl, SettableRegisterBlock,
};
use crate::{Error, PciInterruptController, PciMsiMessage, Result};

// Capability constants
const MESSAGE_CONTROL: usize = 0;
const MESSAGE_CONTROL_TABLE_SIZE: u16 = 0b0111_11111111;
const MESSAGE_CONTROL_FN_MASK: u16 = 1 << 14;
const MESSAGE_CONTROL_MSIX_EN: u16 = 1 << 15;

const TABLE_OFFSET: usize = MESSAGE_CONTROL + 2;
const PBA_OFFSET: usize = TABLE_OFFSET + 4;

const BIR_MASK: u32 = 0b111;
const OFFSET_MASK: u32 = !BIR_MASK;

const MSIX_CAP_SIZE: usize = PBA_OFFSET + 4;

// MSI-X table structure constants
const MSG_ADDR_LO_MASK: u32 = !(0b11);
const VECTOR_CONTROL_MASK: u32 = 1;

pub struct PciMsiXConfig {
    msix_info: PciBarRegionInfo,
    pba_info: PciBarRegionInfo,
}

impl PciMsiXConfig {
    pub fn new(msix_info: PciBarRegionInfo, pba_info: PciBarRegionInfo) -> Self {
        Self {
            msix_info,
            pba_info,
        }
    }

    pub fn build<M, P>(
        self,
        config: &mut PciConfigurationSpace,
        msix_table: M,
        pba_table: P,
    ) -> Result<PciMsiX<M, P>>
    where
        M: MsiXTable,
        P: PbaTable,
    {
        allow_bus_master(config);

        let cap = PciMsiXCapability::new(&self.msix_info, &msix_table, &self.pba_info, &pba_table)?;
        let cap_offset = config.add_capability(&cap)?;

        Ok(PciMsiX::new(
            self.msix_info,
            self.pba_info,
            msix_table,
            pba_table,
            cap_offset,
        ))
    }
}

pub struct PciMsiXCapability {
    message_control: u16,
    msix_offset: u32,
    pba_offset: u32,
}

impl PciMsiXCapability {
    pub fn new(
        msix_info: &PciBarRegionInfo,
        msix_table: &impl MsiXTable,
        pba_info: &PciBarRegionInfo,
        pba_table: &impl PbaTable,
    ) -> Result<Self> {
        let msix_bar = msix_info.bar.into_inner();
        let pba_bar = pba_info.bar.into_inner();

        let msix_offset = msix_info.offset;
        let pba_offset = pba_info.offset;

        // Must be QWORD aligned and must fit to 32bits.
        let offset_mask: u64 = OFFSET_MASK.into();
        if msix_offset | offset_mask != offset_mask {
            return Err(Error::InvalidMsiXTableOffset {
                offset: msix_offset,
            });
        }

        if msix_bar >= 6 {
            return Err(Error::InvalidBarIndex { index: msix_bar });
        }

        // Must be QWORD aligned and must fit to 32bits.
        if pba_offset | offset_mask != offset_mask {
            return Err(Error::InvalidMsiXTableOffset {
                offset: msix_offset,
            });
        }

        if pba_bar >= 6 {
            return Err(Error::InvalidBarIndex { index: pba_bar });
        }

        let num_vectors = msix_table.iter().count();
        // Must not be 0 and must not exceed max size.
        if num_vectors == 0 || num_vectors - 1 > MESSAGE_CONTROL_TABLE_SIZE.into() {
            return Err(Error::InvalidMsiXTableSize { size: num_vectors });
        }

        // Must fit to region.
        if msix_table.raw_bytes().len() as u64 > msix_info.length {
            return Err(Error::InvalidMsiXBarSize {
                size: msix_info.length,
            });
        }

        if pba_table.raw_bytes().len() as u64 > pba_info.length {
            return Err(Error::InvalidMsiXBarSize {
                size: pba_info.length,
            });
        }

        // The spec: "MSI-X Table Size N, which is encoded as N-1".
        let num_vectors = (num_vectors - 1) as u16;

        let msix_offset = msix_offset as u32 | msix_bar as u32;
        let pba_offset = pba_offset as u32 | pba_bar as u32;

        Ok(Self {
            message_control: num_vectors,
            msix_offset,
            pba_offset,
        })
    }
}

impl PciCapability for PciMsiXCapability {
    fn id(&self) -> PciCapabilityId {
        PciCapabilityId::MsiX
    }

    fn size(&self) -> usize {
        MSIX_CAP_SIZE
    }

    fn registers(&self, registers: &mut [u8]) {
        set_word(registers, MESSAGE_CONTROL, self.message_control);
        set_dword(registers, TABLE_OFFSET, self.msix_offset);
        set_dword(registers, PBA_OFFSET, self.pba_offset);
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        writable_bits.fill(0x00);
        set_word(
            writable_bits,
            MESSAGE_CONTROL,
            MESSAGE_CONTROL_MSIX_EN | MESSAGE_CONTROL_FN_MASK,
        );
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct MsiXEntry {
    pub msg_addr: u32,
    pub msg_addr_hi: u32,
    pub msg_data: u32,
    pub vector_control: u32,
}

impl MsiXEntry {
    pub fn msg_addr(&self) -> u64 {
        (self.msg_addr & MSG_ADDR_LO_MASK) as u64 | ((self.msg_addr_hi as u64) << 32)
    }

    pub fn is_masked(&self) -> bool {
        self.vector_control & VECTOR_CONTROL_MASK == 1
    }
}

impl Default for MsiXEntry {
    fn default() -> Self {
        Self {
            msg_addr: 0,
            msg_addr_hi: 0,
            msg_data: 0,
            vector_control: VECTOR_CONTROL_MASK,
        }
    }
}

#[derive(Debug, Default, Copy, Clone)]
#[repr(C, packed)]
pub struct PbaEntry {
    pub pending_bits: u64,
}

/// A Trait for accessing MSI-X tables.
pub trait MsiXTableAccessor {
    type Entry;

    // Table size in bytes.
    fn size_in_bytes(&self) -> usize;

    // Iterators over entries.
    fn iter(&self) -> Iter<'_, Self::Entry>;
    fn iter_mut(&mut self) -> IterMut<'_, Self::Entry>;

    // Get entry at index.
    fn at(&self, index: usize) -> Option<&Self::Entry>;
    fn at_mut(&mut self, index: usize) -> Option<&mut Self::Entry>;

    // Raw access to underlying bytes.
    fn raw_bytes(&self) -> &[u8];
    fn raw_bytes_mut(&mut self) -> &mut [u8];
}

impl<T, E> ReadableRegisterBlock for T
where
    T: MsiXTableAccessor<Entry = E>,
{
    fn registers(&self) -> &[u8] {
        self.raw_bytes()
    }
}

impl<T, E> SettableRegisterBlock for T
where
    T: MsiXTableAccessor<Entry = E>,
{
    fn registers_mut(&mut self) -> &mut [u8] {
        self.raw_bytes_mut()
    }
}

// Blanket for read/set_register() variants.
impl<T, E> RegisterBlockAutoImpl for T where T: MsiXTableAccessor<Entry = E> {}

// Blanket for read/set_register_checked() variants.
impl<T, E> RegisterBlockAccessValidator for T where T: MsiXTableAccessor<Entry = E> {}
impl<T, E> CheckedRegisterBlockAutoImpl for T where T: MsiXTableAccessor<Entry = E> {}

/// A Trait for accessing MSI-X table structure.
pub trait MsiXTable: MsiXTableAccessor<Entry = MsiXEntry> {}

/// A Trait for accessing PBA table structure.
pub trait PbaTable: MsiXTableAccessor<Entry = PbaEntry> {
    fn is_pending(&self, vector: usize) -> bool {
        let (index, shift) = pending_bit_for(vector);

        let Some(pba) = self.at(index) else {
            return false;
        };

        let bit = 1 << shift;

        pba.pending_bits & bit == bit
    }

    fn set_pending_bit(&mut self, vector: PciMsiVector, pending: bool) -> Result<()> {
        let (index, shift) = pending_bit_for(vector);

        let Some(pba) = self.at_mut(index) else {
            return Err(Error::InvalidMsiXVector { vector });
        };

        let bit = 1 << shift;

        if pending {
            pba.pending_bits |= bit;
        } else {
            pba.pending_bits &= !bit;
        }

        Ok(())
    }
}

// MSI-X table blanket implementation for arrays.
impl<const N: usize> MsiXTable for [MsiXEntry; N] {}
impl<const N: usize> MsiXTableAccessor for [MsiXEntry; N] {
    type Entry = MsiXEntry;

    fn size_in_bytes(&self) -> usize {
        self.raw_bytes().len()
    }

    fn iter(&self) -> Iter<'_, Self::Entry> {
        self.as_slice().iter()
    }

    fn iter_mut(&mut self) -> IterMut<'_, Self::Entry> {
        self.as_mut_slice().iter_mut()
    }

    fn raw_bytes(&self) -> &[u8] {
        // SAFETY: an array is guaranteed to be a continuous block of memory.
        let ptr = self.as_ptr() as *const u8;
        let len = mem::size_of_val(self);
        unsafe { slice::from_raw_parts(ptr, len) }
    }

    fn raw_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: an array is guaranteed to be a continuous block of memory.
        let ptr = self.as_mut_ptr() as *mut u8;
        let len = mem::size_of_val(self);
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }

    fn at(&self, index: usize) -> Option<&Self::Entry> {
        match index < self.len() {
            true => Some(&self[index]),
            false => None,
        }
    }

    fn at_mut(&mut self, index: usize) -> Option<&mut Self::Entry> {
        match index < self.len() {
            true => Some(&mut self[index]),
            false => None,
        }
    }
}

// PBA table blanket implementation for arrays.
impl<const N: usize> PbaTable for [PbaEntry; N] {}
impl<const N: usize> MsiXTableAccessor for [PbaEntry; N] {
    type Entry = PbaEntry;

    fn size_in_bytes(&self) -> usize {
        self.raw_bytes().len()
    }

    fn iter(&self) -> Iter<'_, Self::Entry> {
        self.as_slice().iter()
    }

    fn iter_mut(&mut self) -> IterMut<'_, Self::Entry> {
        self.as_mut_slice().iter_mut()
    }

    fn raw_bytes(&self) -> &[u8] {
        // SAFETY: an array is guaranteed to be a continuous block of memory.
        let ptr = self.as_ptr() as *const u8;
        let len = mem::size_of_val(self);
        unsafe { slice::from_raw_parts(ptr, len) }
    }

    fn raw_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: an array is guaranteed to be a continuous block of memory.
        let ptr = self.as_mut_ptr() as *mut u8;
        let len = mem::size_of_val(self);
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }

    fn at(&self, index: usize) -> Option<&Self::Entry> {
        match index < self.len() {
            true => Some(&self[index]),
            false => None,
        }
    }

    fn at_mut(&mut self, index: usize) -> Option<&mut Self::Entry> {
        match index < self.len() {
            true => Some(&mut self[index]),
            false => None,
        }
    }
}

pub struct PciMsiX<MsiXTableT, PbaTableT>
where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    msix_info: PciBarRegionInfo,
    pba_info: PciBarRegionInfo,
    msix_table: MsiXTableT,
    pba_table: PbaTableT,
    cap_offset: PciCapOffset,
}

impl<MsiXTableT, PbaTableT> PciMsiMessageSource for PciMsiX<MsiXTableT, PbaTableT>
where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    fn is_valid_vector(&self, _config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        is_valid_vector(&self.msix_table, vector)
    }

    fn is_enabled(&self, config: &PciConfigurationSpace) -> bool {
        let control = config.read_word(self.cap_offset + MESSAGE_CONTROL);

        control & MESSAGE_CONTROL_MSIX_EN != 0
    }

    fn is_pending(&self, _config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        self.pba_table.is_pending(vector)
    }

    fn set_pending_bit(
        &mut self,
        config: &mut PciConfigurationSpace,
        vector: PciMsiVector,
        pending: bool,
    ) -> Result<()> {
        let _ = config;

        self.pba_table.set_pending_bit(vector, pending)
    }

    fn is_masked(&self, config: &PciConfigurationSpace, vector: PciMsiVector) -> bool {
        let Some(entry) = self.msix_table.at(vector) else {
            return false;
        };

        self.is_fn_masked(config) || entry.is_masked()
    }

    fn get_message_for(
        &mut self,
        _config: &PciConfigurationSpace,
        vector: PciMsiVector,
    ) -> Result<PciMsiMessage> {
        let Some(entry) = self.msix_table.at(vector) else {
            return Err(Error::InvalidMsiXVector { vector });
        };

        Ok(PciMsiMessage {
            address: entry.msg_addr(),
            data: entry.msg_data,
        })
    }
}

impl<MsiXTableT, PbaTableT> PciMsiX<MsiXTableT, PbaTableT>
where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    pub fn new(
        msix_info: PciBarRegionInfo,
        pba_info: PciBarRegionInfo,
        msix_table: MsiXTableT,
        pba_table: PbaTableT,
        cap_offset: PciCapOffset,
    ) -> Self {
        Self {
            msix_info,
            pba_info,
            msix_table,
            pba_table,
            cap_offset,
        }
    }

    pub fn is_fn_masked(&self, config: &PciConfigurationSpace) -> bool {
        let control = config.read_word(self.cap_offset + MESSAGE_CONTROL);

        control & MESSAGE_CONTROL_FN_MASK != 0
    }
}

impl<MsiXTableT, PbaTableT> PciConfigurationSpaceIoHandler for PciMsiX<MsiXTableT, PbaTableT>
where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    type Context<'a> = &'a mut dyn PciInterruptController;
    type R = ();

    fn postprocess_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        offset: usize,
        size: usize,
        interrupt_controller: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let control = control_offset(self);

        if range_overlaps(offset, size, control, 2) && !is_fn_masked(config.read_word(control)) {
            evaluate_pending_interrupts(self, config, *interrupt_controller);
            return Ok(PciHandlerResult::Handled(()));
        }

        Ok(PciHandlerResult::Unhandled)
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum MsiXBarRegion {
    MsiXTable,
    PbaTable,
}

impl<MsiXTableT, PbaTableT> PciBarRegionSet for PciMsiX<MsiXTableT, PbaTableT>
where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    type RegionId = MsiXBarRegion;

    fn info_iter(&self) -> impl Iterator<Item = (Self::RegionId, &PciBarRegionInfo)> {
        let msix = iter::once((MsiXBarRegion::MsiXTable, &self.msix_info));
        let pba = iter::once((MsiXBarRegion::PbaTable, &self.pba_info));

        msix.chain(pba)
    }
}

pub struct MsiXBarHandlerContext<'a> {
    pub config: &'a mut PciConfigurationSpace,
    pub irq_controller: &'a mut dyn PciInterruptController,
}

impl<'a> MsiXBarHandlerContext<'a> {
    pub fn new(
        config: &'a mut PciConfigurationSpace,
        irq_controller: &'a mut impl PciInterruptController,
    ) -> Self {
        Self {
            config,
            irq_controller,
        }
    }
}

impl<MsiXTableT, PbaTableT> PciBarRegionSetHandler for PciMsiX<MsiXTableT, PbaTableT>
where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    type Context<'a> = MsiXBarHandlerContext<'a>;
    type R = Option<PciConfigurationUpdate>;

    fn read_bar(
        &mut self,
        id: Self::RegionId,
        offset: u64,
        data: &mut [u8],
        _context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        match id {
            // FIXME:: offset generics
            MsiXBarRegion::MsiXTable => self
                .msix_table
                .read_register_checked(offset as usize, data)
                .unwrap_or_else(|_| data.fill(0x00)),
            MsiXBarRegion::PbaTable => self
                .pba_table
                .read_register_checked(offset as usize, data)
                .unwrap_or_else(|_| data.fill(0x00)),
        };

        Ok(None)
    }

    fn write_bar(
        &mut self,
        id: Self::RegionId,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        if id == MsiXBarRegion::MsiXTable
            && self
                .msix_table
                .set_register_checked(offset as usize, data)
                .is_ok()
        {
            if targets_vector_control(offset, data.len()) {
                evaluate_pending_interrupts(self, context.config, context.irq_controller);
            }
            // Report MSI message address/data update.
            if targets_message_info(offset, data.len()) {
                let vector = offset_to_vector(offset);
                let msg = self.get_message_for(context.config, vector)?;

                return Ok(Some(PciConfigurationUpdate::MsiXMessage(vector, msg)));
            }
        }

        Ok(None)
    }
}

fn targets_vector_control(offset: u64, size: usize) -> bool {
    let entry_size = entry_size();
    let vector_control_size = mem::size_of_val(&{ MsiXEntry::default().vector_control }) as u64;
    // FIXME: Update rustc and use offset_of
    let vector_control_offs = entry_size - vector_control_size;

    range_overlaps(
        offset % entry_size,
        size as u64,
        vector_control_offs,
        vector_control_size,
    )
}

fn targets_message_data(offset: u64, size: usize) -> bool {
    let entry_size = entry_size();
    let message_data_size = mem::size_of_val(&{ MsiXEntry::default().msg_data }) as u64;
    // FIXME: Update rustc and use offset_of
    let message_data_offs = entry_size - message_data_size - 4;

    range_overlaps(
        offset % entry_size,
        size as u64,
        message_data_offs,
        message_data_size,
    )
}

fn targets_message_address(offset: u64, size: usize) -> bool {
    let message_addr_size = (mem::size_of_val(&{ MsiXEntry::default().msg_addr })
        + mem::size_of_val(&{ MsiXEntry::default().msg_addr_hi }))
        as u64;
    range_overlaps(offset % entry_size(), size as u64, 0, message_addr_size)
}

fn targets_message_info(offset: u64, size: usize) -> bool {
    targets_message_address(offset, size) || targets_message_data(offset, size)
}

fn offset_to_vector(offset: u64) -> usize {
    (offset / entry_size()) as usize
}

fn entry_size() -> u64 {
    mem::size_of::<MsiXEntry>() as u64
}

fn is_valid_vector(msix_table: &impl MsiXTable, vector: PciMsiVector) -> bool {
    vector < msix_table.iter().count()
}

fn find_pending_vector<PbaTableT>(
    pba_table: &PbaTableT,
    since: PciMsiVector,
) -> Option<PciMsiVector>
where
    PbaTableT: PbaTable,
{
    let (entry, bit) = pending_bit_for(since);

    for (i, pba) in pba_table.iter().enumerate().skip(entry) {
        let mut pending = pba.pending_bits;

        // clear previous bits
        pending &= !1u64
            .checked_shl(bit as u32)
            .unwrap_or(0)
            .checked_sub(1)
            .unwrap_or(u64::MAX);

        let bit = pending.trailing_zeros() as usize;

        let size = mem::size_of_val(&pending) * 8;
        if bit < size {
            return Some((i * size) + bit);
        }
    }

    None
}

fn evaluate_pending_interrupts<MsiXTableT, PbaTableT, P: PciInterruptController + ?Sized>(
    msix: &mut PciMsiX<MsiXTableT, PbaTableT>,
    config: &mut PciConfigurationSpace,
    interrupt_controller: &mut P,
) where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    let mut start = 0;
    while let Some(vector) = find_pending_vector(&msix.pba_table, start) {
        if let Ok(PciMsiGenerationResult::Generated(message)) =
            msix.try_generate_message(config, vector)
        {
            interrupt_controller.send_msi(message)
        }
        start = vector + 1;
    }
}

fn is_fn_masked(control: u16) -> bool {
    control & MESSAGE_CONTROL_FN_MASK != 0
}

fn pending_bit_for(vector: usize) -> (usize, u64) {
    let index = vector / 64;
    let shift = (vector % 64) as u64;
    (index, shift)
}

fn control_offset<MsiXTableT, PbaTableT>(msix: &PciMsiX<MsiXTableT, PbaTableT>) -> usize
where
    MsiXTableT: MsiXTable,
    PbaTableT: PbaTable,
{
    msix.cap_offset + MESSAGE_CONTROL
}

#[cfg(test)]
mod tests {
    use std::array::from_fn;
    use std::mem::size_of;

    use crate::bar::PciBarIndex;
    use crate::configuration_space::PciConfigurationSpace;
    use crate::msi::tests::set_bus_master;
    use crate::msi::tests::TestIrqController;

    use super::*;

    fn add_cap(config: &mut PciConfigurationSpace) -> ([MsiXEntry; 32], PciBarIndex, u32, u32) {
        let bar = PciBarIndex::try_from(2).expect("bar index");
        let msix_table = [MsiXEntry::default(); 32];
        let pba_table = [PbaEntry::default(); 1];
        let msix_info = PciBarRegionInfo::new(bar, 0x100, 0x200);
        let pba_info = PciBarRegionInfo::new(bar, 0x300, 0x100);

        config
            .add_capability(
                &PciMsiXCapability::new(&msix_info, &msix_table, &pba_info, &pba_table)
                    .expect("creating msi-x cap failed"),
            )
            .expect("adding cap failed");

        (
            msix_table,
            bar,
            msix_info.offset as u32,
            pba_info.offset as u32,
        )
    }

    // Test MSI-X cap
    #[test]
    fn test_msix_cap_init() {
        let mut config = PciConfigurationSpace::new();

        let (msix_table, bar, msi_offs, pba_offs) = add_cap(&mut config);
        let count = msix_table.len();

        let (header, offset) = config.capability_iter().last().unwrap();

        // cap_id is correct.
        assert_eq!(header.cap_id(), PciCapabilityId::MsiX.into());

        // Table size is count - 1, MSI-X is disabled, function unmasked.
        assert_eq!(config.read_word(offset), (count as u16) - 1);

        let reg = config.read_dword(offset + TABLE_OFFSET);
        // MSI-X table offset is correct
        assert_eq!(reg & OFFSET_MASK, msi_offs);
        // MSI-X table bar is correct
        assert_eq!(reg & BIR_MASK, bar.into_inner() as u32);

        let reg = config.read_dword(offset + PBA_OFFSET);
        // PBA offset is correct
        assert_eq!(reg & OFFSET_MASK, pba_offs);
        // PBA bar is correct
        assert_eq!(reg & BIR_MASK, bar.into_inner() as u32);
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
    fn test_cap_ro_bits() {
        let mut config = PciConfigurationSpace::new();

        add_cap(&mut config);

        let mut expect = [0x00u8; 10];
        expect[1] = 0xC0;
        check_cap_ro_fields(&mut config, &expect);
    }

    // Test MsiXTable blanket implementation
    #[test]
    fn test_msix_table_array() {
        let mut t: [MsiXEntry; 3] = from_fn(|i| {
            let i = i as u32 + 1;
            MsiXEntry {
                msg_addr: i,
                ..Default::default()
            }
        });

        // Test size in bytes
        assert_eq!(t.size_in_bytes(), mem::size_of::<MsiXEntry>() * t.len());

        // test iterator
        assert!(MsiXTableAccessor::iter(&t)
            .enumerate()
            .all(|(i, x)| x.msg_addr == i as u32 + 1));

        // test mut iterator
        MsiXTableAccessor::iter_mut(&mut t)
            .enumerate()
            .for_each(|(i, x)| x.msg_addr_hi = i as u32 + 10);

        assert!(MsiXTableAccessor::iter(&t).enumerate().all(|(i, x)| {
            let i = i as u32;
            i + 1 == x.msg_addr && i + 10 == x.msg_addr_hi
        }));

        // test byte slice
        assert!(t
            .raw_bytes()
            .iter()
            .step_by(size_of::<MsiXEntry>())
            .enumerate()
            .all(|(i, x)| { *x == i as u8 + 1 }));

        // test mut byte slice
        t.raw_bytes_mut()[4..8].copy_from_slice(&[0xFFu8; 4]);
        assert!(t[0].msg_addr_hi == 0xFFFF_FFFF);
    }

    // Test PBA table blanket implementation
    #[test]
    fn test_pba_table_array() {
        let mut t: [PbaEntry; 3] = from_fn(|i| PbaEntry {
            pending_bits: i as u64 + 1,
        });

        // Test size in bytes
        assert_eq!(t.size_in_bytes(), size_of::<PbaEntry>() * t.len());

        // test iterator
        assert!(MsiXTableAccessor::iter(&t)
            .enumerate()
            .all(|(i, x)| x.pending_bits == i as u64 + 1));

        // test mut iterator
        MsiXTableAccessor::iter_mut(&mut t)
            .enumerate()
            .for_each(|(i, x)| x.pending_bits = i as u64 + 10);

        assert!(MsiXTableAccessor::iter(&t)
            .enumerate()
            .all(|(i, x)| { x.pending_bits == i as u64 + 10 }));

        // test byte slice
        assert!(t
            .raw_bytes()
            .iter()
            .step_by(size_of::<PbaEntry>())
            .enumerate()
            .all(|(i, x)| { *x == i as u8 + 10 }));

        // test mut byte slice
        t.raw_bytes_mut()[0..8].copy_from_slice(&[0xFFu8; 8]);
        assert!(t[0].pending_bits == 0xFFFF_FFFF_FFFF_FFFF);
    }

    fn check_pending(pba_table: &impl PbaTable, vectors: &[usize], expect: bool) {
        for v in vectors {
            assert_eq!(pba_table.is_pending(*v), expect);
        }
    }

    fn set_pending_for(pba_table: &mut impl PbaTable, vectors: &[usize], pending: bool) {
        for v in vectors {
            pba_table.set_pending_bit(*v, pending).expect("set pending");
        }
    }

    #[test]
    fn test_pending_bit() {
        let mut pba_table = [PbaEntry::default(); 3];
        let vectors = [0, 63, 64, 127, 128, 191];

        check_pending(&pba_table, &vectors, false);

        // Set pending
        set_pending_for(&mut pba_table, &vectors, true);
        check_pending(&pba_table, &vectors, true);
        // Raw check for one entry
        assert!(pba_table[1].pending_bits & 1 != 0);
        assert!(pba_table[1].pending_bits & (1 << 63) != 0);

        // Clear pending
        set_pending_for(&mut pba_table, &vectors, false);
        check_pending(&pba_table, &vectors, false);
        // Raw check for one entry
        assert!(pba_table[1].pending_bits & 1 == 0);
        assert!(pba_table[1].pending_bits & (1 << 63) == 0);

        // Out of bounds
        assert!(pba_table.set_pending_bit(192, true).is_err());
        assert!(!pba_table.is_pending(192));
    }

    fn msix(
        config: &mut PciConfigurationSpace,
    ) -> (
        PciMsiX<[MsiXEntry; 2], [PbaEntry; 1]>,
        PciBarRegionInfo,
        PciBarRegionInfo,
    ) {
        let bar = PciBarIndex::try_from(1).expect("bar index");
        let msix = PciMsiXConfig::new(
            PciBarRegionInfo::new(bar, 0x1000, 0x1000),
            PciBarRegionInfo::new(bar, 0x2000, 0x1000),
        );
        let msix_table = [MsiXEntry::default(); 2];
        let pba_table = [PbaEntry::default(); 1];

        let msix_info = msix.msix_info.clone();
        let pba_info = msix.pba_info.clone();

        (
            msix.build(config, msix_table, pba_table)
                .expect("building msix failed"),
            msix_info,
            pba_info,
        )
    }

    #[test]
    fn test_msix_pending_bit() {
        let mut config = PciConfigurationSpace::default();
        let (mut msix, _, _) = msix(&mut config);

        msix.set_pending_bit(&mut config, 10, true)
            .expect("set pending_bit");
        assert!(msix.is_pending(&config, 10));
        assert!(msix.pba_table.at(0).unwrap().pending_bits & (1 << 10) == (1 << 10));
        msix.set_pending_bit(&mut config, 10, false)
            .expect("set pending_bit");
        assert!(!msix.is_pending(&config, 10));
        assert!(msix.pba_table.at(0).unwrap().pending_bits & (1 << 10) == 0);
    }

    #[test]
    fn test_find_pending_vector() {
        let mut pba_table = [PbaEntry::default(); 3];
        let mut vectors = [0, 63, 64, 127, 128, 191];
        vectors.sort();

        assert_eq!(find_pending_vector(&pba_table, 0), None);

        set_pending_for(&mut pba_table, &vectors, true);

        let mut start = 0;
        for v in vectors {
            let vector = find_pending_vector(&pba_table, start).unwrap();
            assert_eq!(vector, v);
            start = vector + 1;
        }
        assert_eq!(find_pending_vector(&pba_table, start), None);
    }

    #[test]
    fn test_generate_message() {
        let mut config = PciConfigurationSpace::new();
        let (mut m, _, _) = msix(&mut config);

        // Is not bus master
        assert_eq!(
            m.try_generate_message(&mut config, 0),
            Err(Error::NotBusMaster)
        );

        set_bus_master(&mut config, true);

        // Is not enabled
        assert_eq!(
            m.try_generate_message(&mut config, 0),
            Err(Error::MsiDisabled)
        );

        let control_offset = control_offset(&m);
        config.set_word(
            control_offset,
            MESSAGE_CONTROL_MSIX_EN | MESSAGE_CONTROL_FN_MASK,
        );

        // Is invalid vector
        assert_eq!(
            m.try_generate_message(&mut config, 3),
            Err(Error::InvalidMsiXVector { vector: 3 })
        );

        let entry = m.msix_table.at_mut(0).unwrap();
        // Intentionally incorrectly aligned
        entry.msg_addr = 0xCAFE;
        entry.msg_addr_hi = 0xBAAD;
        entry.msg_data = 0xABBA;
        entry.vector_control = 0;

        // Is fn masked
        assert_eq!(
            m.try_generate_message(&mut config, 0),
            Ok(PciMsiGenerationResult::Masked)
        );
        assert!(m.is_pending(&config, 0));

        m.set_pending_bit(&mut config, 0, false).unwrap();
        m.msix_table.at_mut(0).unwrap().vector_control = 1;
        config.set_word(control_offset, MESSAGE_CONTROL_MSIX_EN);

        // Is vector masked
        assert_eq!(
            m.try_generate_message(&mut config, 0),
            Ok(PciMsiGenerationResult::Masked)
        );
        assert!(m.is_pending(&config, 0));

        m.msix_table.at_mut(0).unwrap().vector_control = 0;
        // Generates message, handles alignment (CAFE -> CAFC)...
        let res = m
            .try_generate_message(&mut config, 0)
            .expect("generate message");
        assert_eq!(
            res,
            PciMsiGenerationResult::Generated(PciMsiMessage {
                address: 0xBAAD_0000_CAFC,
                data: 0xABBA
            })
        );

        // ... and clears pending bit.
        assert!(!m.is_pending(&config, 0));
    }

    fn setup_entries(msix_table: &mut impl MsiXTable) {
        // Configure two MSI-X entries; one of them masked
        let entry = msix_table.at_mut(0).unwrap();
        entry.msg_addr = 0xABBA;
        assert_eq!({ entry.vector_control }, 1);

        let entry = msix_table.at_mut(1).unwrap();
        entry.msg_addr = 0xCAFE;
        entry.vector_control = 0;
    }

    fn postprocess_write_config(
        msix: &mut PciMsiX<[MsiXEntry; 2], [PbaEntry; 1]>,
        config: &mut PciConfigurationSpace,
        controller: &mut TestIrqController,
        access: (usize, usize),
    ) {
        let (offset, size) = access;

        // Set function msi-x masked.
        let mut controller: &mut dyn PciInterruptController = controller;
        msix.postprocess_write_config(config, offset, size, &mut controller)
            .expect("process control");
    }

    #[test]
    fn test_config_write() {
        let mut ic = TestIrqController::default();
        let mut config = PciConfigurationSpace::new();
        let (mut m, _, _) = msix(&mut config);
        let control_offset = control_offset(&m);

        set_bus_master(&mut config, true);

        setup_entries(&mut m.msix_table);

        // Set MSIs pending
        m.set_pending_bit(&mut config, 0, true).unwrap();
        m.set_pending_bit(&mut config, 1, true).unwrap();

        // Function masked -> no MSIs generated.
        config.set_word(
            control_offset,
            MESSAGE_CONTROL_MSIX_EN | MESSAGE_CONTROL_FN_MASK,
        );

        postprocess_write_config(&mut m, &mut config, &mut ic, (control_offset, 2));
        assert!(ic.messages.is_empty());

        // Unmask function.
        config.set_word(control_offset, MESSAGE_CONTROL_MSIX_EN);

        // MSI at 1 should be generated.
        postprocess_write_config(&mut m, &mut config, &mut ic, (control_offset, 2));

        assert_eq!(ic.messages.len(), 1);
        let entry = m.msix_table.at_mut(1).expect("get msi");
        let sent = ic.messages.get(0).unwrap();
        assert_eq!(entry.msg_addr(), sent.address);
        assert_eq!({ entry.msg_data }, sent.data);

        // Unmask at 0 and write config: MSI at 0 should be generated.
        m.msix_table.at_mut(0).unwrap().vector_control = 0;
        postprocess_write_config(&mut m, &mut config, &mut ic, (control_offset, 2));

        assert_eq!(ic.messages.len(), 2);
        let entry = m.msix_table.at_mut(0).expect("get msi");
        let sent = ic.messages.get(1).unwrap();
        assert_eq!(entry.msg_addr(), sent.address);
        assert_eq!({ entry.msg_data }, sent.data);

        // Write - no new MSIs should be generated.
        postprocess_write_config(&mut m, &mut config, &mut ic, (control_offset, 2));
        assert_eq!(ic.messages.len(), 2);
    }

    #[test]
    fn test_read_bar() {
        let mut ic = TestIrqController::default();
        let mut config = PciConfigurationSpace::new();
        let (mut m, msix, pba) = msix(&mut config);

        set_bus_master(&mut config, true);
        setup_entries(&mut m.msix_table);
        m.set_pending_bit(&mut config, 0, true).unwrap();

        let mut context = MsiXBarHandlerContext::new(&mut config, &mut ic);

        let mut data = [0xFFu8; 8];

        // Invalid bar.
        let invalid = PciBarIndex::default();
        let r = m
            .handle_read_bar(invalid, 8, &mut data, &mut context)
            .unwrap();
        assert!(!r.handled());

        // Outside table, but within region.
        let r = m
            .handle_read_bar(msix.bar, msix.offset + 100, &mut data, &mut context)
            .unwrap();
        assert!(r.handled());
        assert_eq!(u64::from_ne_bytes(data), 0);

        // Reads entry 0: vector control.
        let mut data = [0xFFu8; 4];
        let r = m
            .handle_read_bar(msix.bar, msix.offset + 12, &mut data, &mut context)
            .unwrap();
        assert!(r.handled());
        assert_eq!(u32::from_ne_bytes(data), 1);

        // Reads entry 1: addr with 64bit access.
        let mut data = [0xFFu8; 8];
        let r = m
            .handle_read_bar(msix.bar, msix.offset + 16, &mut data, &mut context)
            .unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(u64::from_ne_bytes(data), 0xCAFE);

        // Reads PBA
        let mut data = [0xFFu8; 8];
        let r = m
            .handle_read_bar(pba.bar, pba.offset + 100, &mut data, &mut context)
            .unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(u64::from_ne_bytes(data), 0);

        let mut data = [0xFFu8; 8];
        let r = m
            .handle_read_bar(pba.bar, pba.offset, &mut data, &mut context)
            .unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(u64::from_ne_bytes(data), 1);
    }

    fn bar_context<'a>(
        config: &'a mut PciConfigurationSpace,
        ic: &'a mut impl PciInterruptController,
    ) -> MsiXBarHandlerContext<'a> {
        MsiXBarHandlerContext::new(config, ic)
    }

    #[test]
    fn test_bar_write() {
        let mut ic = TestIrqController::default();
        let mut config = PciConfigurationSpace::new();
        let (mut m, msix, _) = msix(&mut config);

        set_bus_master(&mut config, true);
        setup_entries(&mut m.msix_table);

        config.set_word(control_offset(&m), MESSAGE_CONTROL_MSIX_EN);

        let data = 0xF00Du32;

        // Invalid bar.
        let invalid = PciBarIndex::default();
        let r = m
            .handle_write_bar(
                invalid,
                8,
                &data.to_ne_bytes(),
                &mut bar_context(&mut config, &mut ic),
            )
            .unwrap();
        assert!(!r.handled());

        // Outside table, but within region.
        let r = m
            .handle_write_bar(
                msix.bar,
                msix.offset + 100,
                &data.to_ne_bytes(),
                &mut bar_context(&mut config, &mut ic),
            )
            .unwrap();
        assert!(r.handled());

        // Writes entry 0: vector control.
        m.set_pending_bit(&mut config, 0, true).unwrap();
        assert_eq!({ m.msix_table.at_mut(0).unwrap().vector_control }, 1);

        let r = m
            .handle_write_bar(
                msix.bar,
                msix.offset + 12,
                &0u32.to_ne_bytes(),
                &mut bar_context(&mut config, &mut ic),
            )
            .unwrap();
        assert!(r.handled());
        assert_eq!({ m.msix_table.at_mut(0).unwrap().vector_control }, 0);

        // ... and evaluates interrupts
        assert_eq!(ic.messages.len(), 1);
        let entry = m.msix_table.at_mut(0).unwrap();
        let sent = ic.messages.get(0).unwrap();
        assert_eq!(entry.msg_addr(), sent.address);
        assert_eq!({ entry.msg_data }, sent.data);

        // Writes  entry 1: addr with 64bit access.
        m.set_pending_bit(&mut config, 1, true).unwrap();
        let data = 0xBAAD_DEADBEEFu64;
        let r = m
            .handle_write_bar(
                msix.bar,
                msix.offset + 16,
                &data.to_ne_bytes(),
                &mut bar_context(&mut config, &mut ic),
            )
            .unwrap();
        assert!(r.handled());
        let entry = m.msix_table.at(1).unwrap();
        assert_eq!({ entry.msg_addr_hi }, (data >> 32) as u32);
        assert_eq!({ entry.msg_addr }, data as u32);
        // Didn't change vector_control; interrupts not evaluated.
        assert_eq!(ic.messages.len(), 1);
    }
}
