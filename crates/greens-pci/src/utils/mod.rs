// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::cmp::PartialOrd;
use std::ops;

use crate::{Error, Result};

macro_rules! impl_unsigned_for {
    ($t:ty) => {
        impl Unsigned for $t {
            #[inline]
            fn checked_add(&self, v: &$t) -> Option<$t> {
                <$t>::checked_add(*self, *v)
            }

            #[inline]
            fn is_zero(&self) -> bool {
                *self == 0
            }
        }
    };
}

pub trait Unsigned: Sized + ops::Add<Output = Self> {
    fn checked_add(&self, v: &Self) -> Option<Self>;
    fn is_zero(&self) -> bool;
}

impl_unsigned_for!(usize);
impl_unsigned_for!(u8);
impl_unsigned_for!(u16);
impl_unsigned_for!(u32);
impl_unsigned_for!(u64);

pub mod register_block;

pub fn range_contains<T>(start1: T, size1: T, start2: T, size2: T) -> bool
where
    T: Unsigned + PartialOrd,
{
    if size1.is_zero() || size2.is_zero() {
        return false;
    }

    let Some(end1) = start1.checked_add(&size1) else {
        return false;
    };

    let Some(end2) = start2.checked_add(&size2) else {
        return false;
    };

    start2 >= start1 && end2 <= end1
}

pub fn range_overlaps<T>(start1: T, size1: T, start2: T, size2: T) -> bool
where
    T: Unsigned + PartialOrd,
{
    let Some(end1) = start1.checked_add(&size1) else {
        return false;
    };

    let Some(end2) = start2.checked_add(&size2) else {
        return false;
    };

    !(start1 >= end2 || start2 >= end1)
}

pub fn validate_access(arr: &[u8], offset: usize, data: &[u8]) -> Result<()> {
    validate_bounds(arr, offset, data.len())
}

pub fn set_byte(arr: &mut [u8], offset: usize, data: u8) -> Result<()> {
    set(arr, offset, &data.to_ne_bytes())
}

pub fn set_word(arr: &mut [u8], offset: usize, data: u16) -> Result<()> {
    set(arr, offset, &data.to_ne_bytes())
}

pub fn set_dword(arr: &mut [u8], offset: usize, data: u32) -> Result<()> {
    set(arr, offset, &data.to_ne_bytes())
}

pub fn set(arr: &mut [u8], offset: usize, data: &[u8]) -> Result<()> {
    validate_access(arr, offset, data)?;
    arr[offset..offset + data.len()].copy_from_slice(data);

    Ok(())
}

pub fn read_byte(arr: &[u8], offset: usize) -> Result<u8> {
    let mut data = [0xFFu8; 1];
    read(arr, offset, &mut data)?;
    Ok(data[0])
}

pub fn read_word(arr: &[u8], offset: usize) -> Result<u16> {
    let mut data = [0xFFu8; 2];
    read(arr, offset, &mut data)?;
    Ok(u16::from_ne_bytes(data))
}

pub fn read_dword(arr: &[u8], offset: usize) -> Result<u32> {
    let mut data = [0xFFu8; 4];
    read(arr, offset, &mut data)?;
    Ok(u32::from_ne_bytes(data))
}

pub fn read(arr: &[u8], offset: usize, data: &mut [u8]) -> Result<()> {
    validate_access(arr, offset, data)?;

    data.copy_from_slice(&arr[offset..offset + data.len()]);

    Ok(())
}

// Writes register and considers its writable bits
pub fn write_byte(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: u8) -> Result<()> {
    write(arr, writable_bits, offset, &data.to_ne_bytes())
}

pub fn write_word(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: u16) -> Result<()> {
    write(arr, writable_bits, offset, &data.to_ne_bytes())
}

pub fn write_dword(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: u32) -> Result<()> {
    write(arr, writable_bits, offset, &data.to_ne_bytes())
}

pub fn write(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: &[u8]) -> Result<()> {
    validate_access(arr, offset, data)?;
    validate_access(writable_bits, offset, data)?;

    arr[offset..offset + data.len()]
        .iter_mut()
        .zip(data)
        .zip(writable_bits[offset..offset + data.len()].iter())
        .for_each(|((current, new), writable_bits)| {
            *current = update_register::<u8>(*current, *new, *writable_bits)
        });

    Ok(())
}

pub fn update_register<T>(current: T, new: T, writable_bits: T) -> T
where
    T: Copy + ops::BitAnd<Output = T> + ops::BitOr<Output = T> + ops::Not<Output = T>,
{
    let preserved = current & !writable_bits;
    let changed = new & writable_bits;

    preserved | changed
}

pub fn validate_bounds(registers: &[u8], offset: usize, size: usize) -> Result<()> {
    if offset + size <= registers.len() {
        Ok(())
    } else {
        Err(Error::AccessBounds { offset, size })
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[repr(usize)]
pub enum EndianSwapSize {
    Word = 2,
    Dword = 4,
    Qword = 8,
}

pub fn from_little_endian(data: &mut [u8], max_size: EndianSwapSize) -> Result<()> {
    if data.len() > max_size as usize {
        return Err(Error::InvalidIoSize { size: data.len() });
    }

    match data.len() {
        1 => Ok(()),
        2 => {
            let word = u16::from_le_bytes(data[0..data.len()].try_into().unwrap());
            data.copy_from_slice(&word.to_ne_bytes());
            Ok(())
        }
        4 => {
            let dword = u32::from_le_bytes(data[0..data.len()].try_into().unwrap());
            data.copy_from_slice(&dword.to_ne_bytes());
            Ok(())
        }
        8 => {
            let qword = u64::from_le_bytes(data[0..data.len()].try_into().unwrap());
            data.copy_from_slice(&qword.to_ne_bytes());
            Ok(())
        }
        _ => Err(Error::InvalidIoSize { size: data.len() }),
    }
}

pub fn to_little_endian(data: &mut [u8], max_size: EndianSwapSize) -> Result<()> {
    if data.len() > max_size as usize {
        return Err(Error::InvalidIoSize { size: data.len() });
    }

    match data.len() {
        1 => Ok(()),
        2 => {
            let word = u16::from_ne_bytes(data[0..data.len()].try_into().unwrap());
            data.copy_from_slice(&word.to_le_bytes());
            Ok(())
        }
        4 => {
            let dword = u32::from_ne_bytes(data[0..data.len()].try_into().unwrap());
            data.copy_from_slice(&dword.to_le_bytes());
            Ok(())
        }
        8 => {
            let qword = u64::from_ne_bytes(data[0..data.len()].try_into().unwrap());
            data.copy_from_slice(&qword.to_le_bytes());
            Ok(())
        }
        _ => Err(Error::InvalidIoSize { size: data.len() }),
    }
}

pub fn access_data_window(
    access_offset: usize,
    access_size: usize,
    cap_offset: usize,
    cap_size: usize,
) -> Option<(usize, usize)> {
    let Some(access_end) = access_offset.checked_add(access_size) else {
        // Overflow
        return None;
    };

    let Some(cap_end) = cap_offset.checked_add(cap_size) else {
        // Overflow
        return None;
    };

    // Out of bounds: right
    if access_offset >= cap_end {
        return None;
    }

    // Window start; handle left truncation
    let start = cap_offset.saturating_sub(access_offset);

    // Out of bounds: left
    if start >= access_size {
        return None;
    }

    // Window end; handle right truncation
    let mut end = access_end.checked_sub(cap_end).unwrap_or(access_size);
    if end == 0 {
        end = access_size
    }

    Some((start, end))
}

pub fn offset_within_range(addr: usize, range_start: usize, range_size: usize) -> Option<usize> {
    let offset = addr.checked_sub(range_start)?;
    if offset >= range_size {
        return None;
    }
    Some(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_data_window_truncate_left() {
        // access data is array: 0..N
        // access begins at 2, size 4
        // cap begins at 4, size 4
        //
        // offset    0123456789ABCDEF
        // cap           ====
        // access      ----
        // expect        ~~
        assert_eq!(access_data_window(2, 4, 4, 4), Some((2, 4)));
    }

    #[test]
    fn test_access_data_window_full() {
        // CASE 1:
        // offset    0123456789ABCDEF
        // cap           ====
        // access        ----
        // expect        ~~~~
        assert_eq!(access_data_window(4, 4, 4, 4), Some((0, 4)));

        // CASE 2:
        // offset    0123456789ABCDEF
        // cap           ====
        // access        --
        // expect        ~~
        assert_eq!(access_data_window(4, 2, 4, 4), Some((0, 2)));

        // CASE 3:
        // offset    0123456789ABCDEF
        // cap           ====
        // access          --
        // expect          ~~
        assert_eq!(access_data_window(6, 2, 4, 4), Some((0, 2)));
    }

    #[test]
    fn test_access_data_window_truncate_right() {
        // access data is array: 0..N
        // access begins at 6, size 4
        // cap begins at 4, size 4
        //
        // offset    0123456789ABCDEF
        // cap           ====
        // access          ----
        // expect          ~~
        assert_eq!(access_data_window(6, 4, 4, 4), Some((0, 2)));
    }

    #[test]
    fn test_access_data_window_outside() {
        // Left
        assert_eq!(access_data_window(0, 2, 4, 4), None);
        // Right
        assert_eq!(access_data_window(8, 2, 4, 4), None);
    }

    #[test]
    fn test_access_data_window_overflow() {
        // Overflow: access
        assert_eq!(access_data_window(usize::MAX, 4, 4, 4), None);
        assert_eq!(access_data_window(4, usize::MAX, 4, 4), None);
        // Overflow: cap
        assert_eq!(access_data_window(2, 4, usize::MAX, 4), None);
        assert_eq!(access_data_window(2, 4, 4, usize::MAX), None);
    }

    #[test]
    fn test_offset_within_range() {
        assert_eq!(offset_within_range(0, 0, 1), Some(0));
        assert_eq!(offset_within_range(2, 2, 2), Some(0));
        assert_eq!(offset_within_range(3, 2, 2), Some(1));
    }

    #[test]
    fn test_offset_within_range_outside() {
        // Left
        assert_eq!(offset_within_range(1, 2, 2), None);
        // Right
        assert_eq!(offset_within_range(4, 2, 2), None);
    }

    #[test]
    fn test_offset_within_range_underflow() {
        assert_eq!(offset_within_range(4, usize::MAX, 2), None);
    }
}
