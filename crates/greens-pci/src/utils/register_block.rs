// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::ops;

use crate::{Error, Result};

pub fn set_byte(arr: &mut [u8], offset: usize, data: u8) {
    set(arr, offset, &data.to_ne_bytes())
}

pub fn set_word(arr: &mut [u8], offset: usize, data: u16) {
    set(arr, offset, &data.to_ne_bytes())
}

pub fn set_dword(arr: &mut [u8], offset: usize, data: u32) {
    set(arr, offset, &data.to_ne_bytes())
}

pub fn set_qword(arr: &mut [u8], offset: usize, data: u64) {
    set(arr, offset, &data.to_ne_bytes())
}

pub fn set(arr: &mut [u8], offset: usize, data: &[u8]) {
    arr[offset..offset + data.len()].copy_from_slice(data);
}

pub fn read_byte(arr: &[u8], offset: usize) -> u8 {
    let mut data = [0xFFu8; 1];
    read(arr, offset, &mut data);
    data[0]
}

pub fn read_word(arr: &[u8], offset: usize) -> u16 {
    let mut data = [0xFFu8; 2];
    read(arr, offset, &mut data);
    u16::from_ne_bytes(data)
}

pub fn read_dword(arr: &[u8], offset: usize) -> u32 {
    let mut data = [0xFFu8; 4];
    read(arr, offset, &mut data);
    u32::from_ne_bytes(data)
}

pub fn read_qword(arr: &[u8], offset: usize) -> u64 {
    let mut data = [0xFFu8; 8];
    read(arr, offset, &mut data);
    u64::from_ne_bytes(data)
}

pub fn read(arr: &[u8], offset: usize, data: &mut [u8]) {
    data.copy_from_slice(&arr[offset..offset + data.len()]);
}

// Writes register and considers its writable bits
pub fn write_byte(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: u8) {
    write(arr, writable_bits, offset, &data.to_ne_bytes())
}

pub fn write_word(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: u16) {
    write(arr, writable_bits, offset, &data.to_ne_bytes())
}

pub fn write_dword(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: u32) {
    write(arr, writable_bits, offset, &data.to_ne_bytes())
}

pub fn write_qword(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: u64) {
    write(arr, writable_bits, offset, &data.to_ne_bytes())
}

pub fn write(arr: &mut [u8], writable_bits: &[u8], offset: usize, data: &[u8]) {
    arr[offset..offset + data.len()]
        .iter_mut()
        .zip(data)
        .zip(writable_bits[offset..offset + data.len()].iter())
        .for_each(|((current, new), writable_bits)| {
            *current = update_register::<u8>(*current, *new, *writable_bits)
        });
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

pub trait ReadableRegisterBlock {
    fn registers(&self) -> &[u8];
}

pub trait SettableRegisterBlock {
    fn registers_mut(&mut self) -> &mut [u8];
}

pub trait WritableRegisterBlock {
    fn writable_bits(&self) -> &[u8];
    fn write_context(&mut self) -> (&mut [u8], &[u8]);
}

pub trait RegisterBlockReader: ReadableRegisterBlock {
    fn read_register_byte(&self, offset: usize) -> u8 {
        let mut data = [0xFFu8; 1];
        self.read_register(offset, &mut data);
        data[0]
    }

    fn read_register_word(&self, offset: usize) -> u16 {
        let mut data = [0xFFu8; 2];
        self.read_register(offset, &mut data);
        u16::from_ne_bytes(data)
    }

    fn read_register_dword(&self, offset: usize) -> u32 {
        let mut data = [0xFFu8; 4];
        self.read_register(offset, &mut data);
        u32::from_ne_bytes(data)
    }

    fn read_register(&self, offset: usize, data: &mut [u8]) {
        read(self.registers(), offset, data)
    }
}

pub trait RegisterBlockReader64: RegisterBlockReader {
    fn read_register_qword(&self, offset: usize) -> u64 {
        let mut data = [0xFFu8; 8];
        self.read_register(offset, &mut data);
        u64::from_ne_bytes(data)
    }
}

pub trait RegisterBlockSetter: SettableRegisterBlock {
    fn set_register_byte(&mut self, offset: usize, data: u8) {
        self.set_register(offset, &data.to_ne_bytes())
    }

    fn set_register_word(&mut self, offset: usize, data: u16) {
        self.set_register(offset, &data.to_ne_bytes())
    }

    fn set_register_dword(&mut self, offset: usize, data: u32) {
        self.set_register(offset, &data.to_ne_bytes())
    }

    fn set_register(&mut self, offset: usize, data: &[u8]) {
        self.registers_mut()[offset..offset + data.len()].copy_from_slice(data);
    }
}

pub trait RegisterBlockSetter64: RegisterBlockSetter {
    fn set_register_qword(&mut self, offset: usize, data: u64) {
        self.set_register(offset, &data.to_ne_bytes())
    }
}

pub trait RegisterBlockWriter: WritableRegisterBlock {
    fn write_register_byte(&mut self, offset: usize, data: u8) {
        self.write_register(offset, &data.to_ne_bytes())
    }

    fn write_register_word(&mut self, offset: usize, data: u16) {
        self.write_register(offset, &data.to_ne_bytes())
    }

    fn write_register_dword(&mut self, offset: usize, data: u32) {
        self.write_register(offset, &data.to_ne_bytes())
    }

    fn write_register(&mut self, offset: usize, data: &[u8]) {
        let (registers, writable_bits) = self.write_context();

        assert_eq!(registers.len(), writable_bits.len());

        write(registers, writable_bits, offset, data)
    }
}

pub trait RegisterBlockWriter64: RegisterBlockWriter {
    fn write_register_qword(&mut self, offset: usize, data: u64) {
        self.write_register(offset, &data.to_ne_bytes())
    }
}

pub trait RegisterBlockAccessValidator: ReadableRegisterBlock {
    fn validate_access(&self, offset: usize, data: &[u8]) -> Result<()> {
        validate_bounds(self.registers(), offset, data.len())
    }
}

pub trait CheckedRegisterBlockReader: RegisterBlockAccessValidator {
    fn read_register_byte_checked(&self, offset: usize) -> Result<u8> {
        let mut data = [0xFFu8; 1];
        self.read_register_checked(offset, &mut data)?;
        Ok(data[0])
    }

    fn read_register_word_checked(&self, offset: usize) -> Result<u16> {
        let mut data = [0xFFu8; 2];
        self.read_register_checked(offset, &mut data)?;
        Ok(u16::from_ne_bytes(data))
    }

    fn read_register_dword_checked(&self, offset: usize) -> Result<u32> {
        let mut data = [0xFFu8; 4];
        self.read_register_checked(offset, &mut data)?;
        Ok(u32::from_ne_bytes(data))
    }

    fn read_register_checked(&self, offset: usize, data: &mut [u8]) -> Result<()> {
        self.validate_access(offset, data)?;
        read(self.registers(), offset, data);
        Ok(())
    }
}

pub trait CheckedRegisterBlockReader64: CheckedRegisterBlockReader {
    fn read_register_qword_checked(&self, offset: usize) -> Result<u64> {
        let mut data = [0xFFu8; 8];
        self.read_register_checked(offset, &mut data)?;
        Ok(u64::from_ne_bytes(data))
    }
}

pub trait CheckedRegisterBlockSetter: RegisterBlockAccessValidator + SettableRegisterBlock {
    fn set_register_byte_checked(&mut self, offset: usize, data: u8) -> Result<()> {
        self.set_register_checked(offset, &data.to_ne_bytes())
    }

    fn set_register_word_checked(&mut self, offset: usize, data: u16) -> Result<()> {
        self.set_register_checked(offset, &data.to_ne_bytes())
    }

    fn set_register_dword_checked(&mut self, offset: usize, data: u32) -> Result<()> {
        self.set_register_checked(offset, &data.to_ne_bytes())
    }

    fn set_register_checked(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        self.validate_access(offset, data)?;
        set(self.registers_mut(), offset, data);
        Ok(())
    }
}

pub trait CheckedRegisterBlockSetter64: CheckedRegisterBlockSetter {
    fn set_register_qword_checked(&mut self, offset: usize, data: u64) -> Result<()> {
        self.set_register_checked(offset, &data.to_ne_bytes())
    }
}

pub trait RegisterBlockWriteAccessValidator: WritableRegisterBlock + ReadableRegisterBlock {
    fn validate_write_access(&self, offset: usize, data: &[u8]) -> Result<()> {
        validate_bounds(self.registers(), offset, data.len())?;
        validate_bounds(self.writable_bits(), offset, data.len())
    }
}

pub trait CheckedRegisterBlockWriter: RegisterBlockWriteAccessValidator {
    fn write_register_byte_checked(&mut self, offset: usize, data: u8) -> Result<()> {
        self.write_register_checked(offset, &data.to_ne_bytes())
    }

    fn write_register_word_checked(&mut self, offset: usize, data: u16) -> Result<()> {
        self.write_register_checked(offset, &data.to_ne_bytes())
    }

    fn write_register_dword_checked(&mut self, offset: usize, data: u32) -> Result<()> {
        self.write_register_checked(offset, &data.to_ne_bytes())
    }

    fn write_register_checked(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        self.validate_write_access(offset, data)?;
        let (registers, writable_bits) = self.write_context();
        write(registers, writable_bits, offset, data);
        Ok(())
    }
}

pub trait CheckedRegisterBlockWriter64: CheckedRegisterBlockWriter {
    fn write_register_qword_checked(&mut self, offset: usize, data: u64) -> Result<()> {
        self.write_register_checked(offset, &data.to_ne_bytes())
    }
}

// Marker traits for blanket implementations
pub trait RegisterBlockAutoImpl {}
pub trait RegisterBlock64AutoImpl {}
pub trait CheckedRegisterBlockAutoImpl {}

impl<T> RegisterBlockReader for T where T: ReadableRegisterBlock + RegisterBlockAutoImpl {}
impl<T> RegisterBlockSetter for T where T: SettableRegisterBlock + RegisterBlockAutoImpl {}
impl<T> RegisterBlockWriter for T where T: WritableRegisterBlock + RegisterBlockAutoImpl {}

impl<T> RegisterBlockReader64 for T where T: RegisterBlockReader + RegisterBlock64AutoImpl {}
impl<T> RegisterBlockSetter64 for T where T: RegisterBlockSetter + RegisterBlock64AutoImpl {}
impl<T> RegisterBlockWriter64 for T where T: RegisterBlockWriter + RegisterBlock64AutoImpl {}

impl<T> CheckedRegisterBlockReader for T where
    T: RegisterBlockAccessValidator + CheckedRegisterBlockAutoImpl
{
}
impl<T> CheckedRegisterBlockSetter for T where
    T: SettableRegisterBlock + RegisterBlockAccessValidator + CheckedRegisterBlockAutoImpl
{
}
impl<T> CheckedRegisterBlockWriter for T where
    T: RegisterBlockWriteAccessValidator + CheckedRegisterBlockAutoImpl
{
}

impl<T> CheckedRegisterBlockReader64 for T where
    T: CheckedRegisterBlockReader + CheckedRegisterBlockAutoImpl + RegisterBlock64AutoImpl
{
}
impl<T> CheckedRegisterBlockSetter64 for T where
    T: CheckedRegisterBlockSetter + CheckedRegisterBlockAutoImpl + RegisterBlock64AutoImpl
{
}
impl<T> CheckedRegisterBlockWriter64 for T where
    T: CheckedRegisterBlockWriter + CheckedRegisterBlockAutoImpl + RegisterBlock64AutoImpl
{
}

// For generating alternative accessors: read_register_byte -> read_byte
#[macro_export]
macro_rules! impl_accessors {
    (
        $(
            $kind:ident ( $($args:tt)* )
        )*
    ) => {
        $(
            $crate::impl_accessors!(@expand $kind ( $($args)* ));
        )*
    };

    (@expand load($name:ident, $target:ident, $ret:ty)) => {
        #[inline]
        pub fn $name(&self, offset: usize) -> $ret {
            self.$target(offset)
        }
    };

    (@expand load_arg($name:ident, $target:ident, $arg:ty)) => {
        #[inline]
        pub fn $name(&self, offset: usize, data: $arg) {
            self.$target(offset, data)
        }
    };

    (@expand load_checked($name:ident, $target:ident, $arg:ty, $ret:ty)) => {
        #[inline]
        pub fn $name(&self, offset: usize, data: $arg) -> $ret {
            self.$target(offset, data)
        }
    };

    (@expand store($name:ident, $target:ident, $arg:ty)) => {
        #[inline]
        pub fn $name(&mut self, offset: usize, data: $arg) {
            self.$target(offset, data)
        }
    };

    (@expand store_checked($name:ident, $target:ident, $arg:ty, $ret:ty)) => {
        #[inline]
        pub fn $name(&mut self, offset: usize, data: $arg) -> $ret {
            self.$target(offset, data)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BLOCK_SIZE: usize = 16;

    struct TestRegBlock {
        registers: [u8; TEST_BLOCK_SIZE],
        writable_bits: [u8; TEST_BLOCK_SIZE],
    }

    impl TestRegBlock {
        fn new() -> Self {
            let mut registers = [0x00u8; TEST_BLOCK_SIZE];
            let mut writable_bits = [0x00u8; TEST_BLOCK_SIZE];

            // 1, 2, 3 ...
            for (index, value) in registers.iter_mut().enumerate() {
                *value = index as u8 + 1;
            }

            // [00, FF, F0, FF, 00, F0, 00, FF, F0, FF, 00, F0, 00, FF, F0, FF]
            for (index, value) in writable_bits.iter_mut().enumerate() {
                let index = index + 1;
                if index % 3 == 0 {
                    *value = 0xF0
                } else if index % 2 == 0 {
                    *value = 0xFF;
                } else {
                    *value = 0x00;
                }
            }

            Self {
                registers,
                writable_bits,
            }
        }
    }

    // For read support.
    impl ReadableRegisterBlock for TestRegBlock {
        fn registers(&self) -> &[u8] {
            &self.registers
        }
    }

    // For set support.
    impl SettableRegisterBlock for TestRegBlock {
        fn registers_mut(&mut self) -> &mut [u8] {
            &mut self.registers
        }
    }

    // For write support.
    impl WritableRegisterBlock for TestRegBlock {
        fn writable_bits(&self) -> &[u8] {
            &self.writable_bits
        }

        fn write_context(&mut self) -> (&mut [u8], &[u8]) {
            (&mut self.registers, &self.writable_bits)
        }
    }

    // For checked read/set/write.
    impl RegisterBlockAccessValidator for TestRegBlock {}
    impl RegisterBlockWriteAccessValidator for TestRegBlock {}

    // And blankets for all.
    impl RegisterBlockAutoImpl for TestRegBlock {}
    impl RegisterBlock64AutoImpl for TestRegBlock {}
    impl CheckedRegisterBlockAutoImpl for TestRegBlock {}

    #[test]
    fn test_reg_read() {
        let rb = TestRegBlock::new();

        // read byte
        assert_eq!(rb.read_register_byte(0), rb.registers[0]);
        assert_eq!(rb.read_register_byte(15), rb.registers[15]);

        // read word
        assert_eq!(
            rb.read_register_word(0),
            u16::from_ne_bytes(rb.registers[..2].try_into().unwrap())
        );
        assert_eq!(
            rb.read_register_word(14),
            u16::from_ne_bytes(rb.registers[14..].try_into().unwrap())
        );

        // read dword
        assert_eq!(
            rb.read_register_dword(0),
            u32::from_ne_bytes(rb.registers[..4].try_into().unwrap())
        );
        assert_eq!(
            rb.read_register_dword(12),
            u32::from_ne_bytes(rb.registers[12..].try_into().unwrap())
        );

        // read qword
        assert_eq!(
            rb.read_register_qword(0),
            u64::from_ne_bytes(rb.registers[..8].try_into().unwrap())
        );
        assert_eq!(
            rb.read_register_qword(8),
            u64::from_ne_bytes(rb.registers[8..].try_into().unwrap())
        );

        // arbitrary sized read
        let mut data = [0u8; 5];
        rb.read_register(3, &mut data);
        assert_eq!(data, rb.registers[3..8]);
    }

    #[test]
    fn test_reg_set() {
        // set byte
        let mut rb = TestRegBlock::new();
        let mut expect = 0;

        rb.set_register_byte(0, expect);
        assert_eq!(rb.registers[0], expect);

        expect = 100;
        rb.set_register_byte(15, expect);
        assert_eq!(rb.registers[15], expect);

        // set word
        let mut rb = TestRegBlock::new();
        let mut expect = 0;

        rb.set_register_word(0, expect);
        assert_eq!(
            u16::from_ne_bytes(rb.registers[..2].try_into().unwrap()),
            expect
        );

        expect = 0xABBA;
        rb.set_register_word(14, expect);
        assert_eq!(
            u16::from_ne_bytes(rb.registers[14..].try_into().unwrap()),
            expect
        );

        // set dword
        let mut rb = TestRegBlock::new();
        let mut expect = 0;

        rb.set_register_dword(0, expect);
        assert_eq!(
            u32::from_ne_bytes(rb.registers[..4].try_into().unwrap()),
            expect
        );

        expect = 0xABBA_CAFE;
        rb.set_register_dword(12, expect);
        assert_eq!(
            u32::from_ne_bytes(rb.registers[12..].try_into().unwrap()),
            expect
        );

        // set qword
        let mut rb = TestRegBlock::new();
        let mut expect = 0;

        rb.set_register_qword(0, expect);
        assert_eq!(
            u64::from_ne_bytes(rb.registers[..8].try_into().unwrap()),
            expect
        );

        expect = 0xABBA_CAFE_BAAD_F00D;
        rb.set_register_qword(8, expect);
        assert_eq!(
            u64::from_ne_bytes(rb.registers[8..].try_into().unwrap()),
            expect
        );

        // arbitrary sized set
        let mut rb = TestRegBlock::new();
        let data = [4u8; 5];
        rb.set_register(3, &data);
        assert_eq!(data, rb.registers[3..8]);
    }

    #[test]
    fn test_reg_write() {
        // write byte
        let mut rb = TestRegBlock::new();

        // R/O
        let mut expect = rb.registers[0];
        rb.write_register_byte(0, 0);
        assert_eq!(rb.registers[0], expect);

        // R/W
        expect = 0xFF;
        rb.write_register_byte(1, expect);
        assert_eq!(rb.registers[1], expect);

        // Partially R/W
        rb.write_register_byte(2, 0xFF);
        assert_eq!(rb.registers[2], 0xF3);

        // write word
        let mut rb = TestRegBlock::new();
        let write = 0xABBA;
        let mask = u16::from_ne_bytes(rb.writable_bits[..2].try_into().unwrap());
        let current = u16::from_ne_bytes(rb.registers[..2].try_into().unwrap());
        let expect = update_register(current, write, mask);

        rb.write_register_word(0, write);
        assert_eq!(
            u16::from_ne_bytes(rb.registers[..2].try_into().unwrap()),
            expect
        );

        let mask = u16::from_ne_bytes(rb.writable_bits[14..].try_into().unwrap());
        let current = u16::from_ne_bytes(rb.registers[14..].try_into().unwrap());
        let expect = update_register(current, write, mask);
        rb.write_register_word(14, write);
        assert_eq!(
            u16::from_ne_bytes(rb.registers[14..].try_into().unwrap()),
            expect
        );

        // write dword
        let mut rb = TestRegBlock::new();
        let write = 0xABBA_CAFE;
        let mask = u32::from_ne_bytes(rb.writable_bits[12..].try_into().unwrap());
        let current = u32::from_ne_bytes(rb.registers[12..].try_into().unwrap());
        let expect = update_register(current, write, mask);

        rb.write_register_dword(12, write);
        assert_eq!(
            u32::from_ne_bytes(rb.registers[12..].try_into().unwrap()),
            expect
        );

        // write qword
        let write: u64 = 0xABBA_CAFE_BAAD_F00D;
        let mask = u64::from_ne_bytes(rb.writable_bits[8..].try_into().unwrap());
        let current = u64::from_ne_bytes(rb.registers[8..].try_into().unwrap());
        let expect = update_register(current, write, mask);

        rb.write_register_qword(8, write);
        assert_eq!(
            u64::from_ne_bytes(rb.registers[8..].try_into().unwrap()),
            expect
        );

        // arbitrary sized write
        let mut rb = TestRegBlock::new();
        let data = [4u8; 5];
        let expect = [4, 5, 6, 7, 4];
        rb.write_register(3, &data);
        assert_eq!(expect, rb.registers[3..8]);
    }

    #[test]
    fn test_reg_read_checked() {
        let rb = TestRegBlock::new();
        // byte
        assert_eq!(rb.read_register_byte_checked(15).unwrap(), rb.registers[15]);
        assert!(rb.read_register_byte_checked(16).is_err());

        // word
        assert_eq!(
            rb.read_register_word(14),
            u16::from_ne_bytes(rb.registers[14..].try_into().unwrap())
        );
        assert!(rb.read_register_word_checked(15).is_err());

        // dword
        assert_eq!(
            rb.read_register_dword(12),
            u32::from_ne_bytes(rb.registers[12..].try_into().unwrap())
        );
        assert!(rb.read_register_dword_checked(13).is_err());

        // qword
        assert_eq!(
            rb.read_register_qword(8),
            u64::from_ne_bytes(rb.registers[8..].try_into().unwrap())
        );
        assert!(rb.read_register_qword_checked(9).is_err());
    }

    #[test]
    fn test_reg_set_checked() {
        // set byte
        let mut rb = TestRegBlock::new();
        let expect = 0xAB;
        rb.set_register_byte_checked(15, expect).unwrap();
        assert_eq!(rb.registers[15], expect);
        assert!(rb.set_register_byte_checked(16, expect).is_err());

        // set word
        let mut rb = TestRegBlock::new();
        let expect = 0xABBA;
        rb.set_register_word_checked(14, expect).unwrap();
        assert_eq!(
            u16::from_ne_bytes(rb.registers[14..].try_into().unwrap()),
            expect
        );
        assert!(rb.set_register_word_checked(15, expect).is_err());

        // set dword
        let mut rb = TestRegBlock::new();
        let expect = 0xABBA_CAFE;
        rb.set_register_dword_checked(12, expect).unwrap();
        assert_eq!(
            u32::from_ne_bytes(rb.registers[12..].try_into().unwrap()),
            expect
        );
        assert!(rb.set_register_dword_checked(13, expect).is_err());

        // set qword
        let mut rb = TestRegBlock::new();
        let expect = 0xABBA_CAFE_BAAD_F00D;
        rb.set_register_qword_checked(8, expect).unwrap();
        assert_eq!(
            u64::from_ne_bytes(rb.registers[8..].try_into().unwrap()),
            expect
        );
        assert!(rb.set_register_qword_checked(9, expect).is_err());
    }
}
