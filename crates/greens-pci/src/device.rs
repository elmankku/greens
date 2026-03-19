use crate::function::PciConfigurationUpdate;
// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::registers::{
    PCI_CONFIGURATION_SPACE_MAX_IO_SIZE, PCI_CONFIGURATION_SPACE_SIZE, PCI_MAX_DEVICE_FUNCTIONS,
};
use crate::utils::{EndianSwapSize, from_little_endian, to_little_endian};
use crate::{Error, Result};

pub trait PciDevice {
    fn read_fn_config(&mut self, function: usize, offset: usize, data: &mut [u8]) -> Result<()>;
    fn write_fn_config(
        &mut self,
        function: usize,
        offset: usize,
        data: &[u8],
    ) -> Result<Option<PciConfigurationUpdate>>;

    fn read_config(&mut self, offset: usize, data: &mut [u8]) -> Result<()> {
        let Some((function, offset)) = function_index_and_offset(offset) else {
            return Err(Error::AccessBounds {
                offset,
                size: data.len(),
            });
        };
        self.read_fn_config(function, offset, data)
    }
    fn write_config(
        &mut self,
        offset: usize,
        data: &[u8],
    ) -> Result<Option<PciConfigurationUpdate>> {
        let Some((function, offset)) = function_index_and_offset(offset) else {
            return Err(Error::AccessBounds {
                offset,
                size: data.len(),
            });
        };
        self.write_fn_config(function, offset, data)
    }

    fn read_config_le(&mut self, offset: usize, data: &mut [u8]) -> Result<()> {
        self.read_config(offset, data)?;

        to_little_endian(data, EndianSwapSize::Dword)
    }

    fn write_config_le(
        &mut self,
        offset: usize,
        data: &[u8],
    ) -> Result<Option<PciConfigurationUpdate>> {
        let size = data.len();
        if size > PCI_CONFIGURATION_SPACE_MAX_IO_SIZE {
            return Err(Error::InvalidIoSize { size });
        }

        let mut input = [0xFFu8; PCI_CONFIGURATION_SPACE_MAX_IO_SIZE];
        let input = &mut input[..size];
        input.copy_from_slice(data);
        from_little_endian(input, EndianSwapSize::Dword)?;

        self.write_config(offset, input)
    }

    fn read_mmio(&mut self, address: u64, data: &mut [u8]) -> Result<()>;
    fn write_mmio(&mut self, address: u64, data: &[u8]) -> Result<Option<PciConfigurationUpdate>>;

    fn read_pio(&mut self, port: u16, data: &mut [u8]) -> Result<()>;
    fn write_pio(&mut self, port: u16, data: &mut [u8]) -> Result<Option<PciConfigurationUpdate>>;
}

fn function_index_and_offset(offset: usize) -> Option<(usize, usize)> {
    let function = offset / PCI_CONFIGURATION_SPACE_SIZE;
    if function > PCI_MAX_DEVICE_FUNCTIONS {
        return None;
    }
    Some((function, offset % PCI_CONFIGURATION_SPACE_SIZE))
}
