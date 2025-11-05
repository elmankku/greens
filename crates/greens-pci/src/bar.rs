// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::ops;

use crate::registers::{
    NUM_BAR_REGS, PCI_BAR_IO_INDICATOR_MASK, PCI_BAR_MEM_32_INDICATOR_MASK,
    PCI_BAR_MEM_64_INDICATOR_MASK, PCI_BAR_MEM_PREFETCHABLE_MASK,
};
use crate::{Error, Result};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PciBarPrefetchable {
    NotPrefetchable = 0x00u32,
    Prefetchable = PCI_BAR_MEM_PREFETCHABLE_MASK,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PciBarType {
    Io,
    Memory32Bit(PciBarPrefetchable),
    Memory64Bit(PciBarPrefetchable),
}

impl PciBarType {
    pub fn bits(&self) -> u32 {
        match &self {
            PciBarType::Io => PCI_BAR_IO_INDICATOR_MASK,
            PciBarType::Memory32Bit(prefetchable) => {
                PCI_BAR_MEM_32_INDICATOR_MASK | *prefetchable as u32
            }
            PciBarType::Memory64Bit(prefetchable) => {
                PCI_BAR_MEM_64_INDICATOR_MASK | *prefetchable as u32
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PciBar {
    address: Option<u64>,
    size: u64,
    index: PciBarIndex,
    region_type: PciBarType,
}

impl PciBar {
    pub fn new(
        address: Option<u64>,
        size: u64,
        index: PciBarIndex,
        region_type: PciBarType,
    ) -> Self {
        Self {
            address,
            size,
            index,
            region_type,
        }
    }

    pub fn is_mem(&self) -> bool {
        !matches!(self.region_type, PciBarType::Io)
    }

    pub fn is_io(&self) -> bool {
        !self.is_mem()
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn address(&self) -> Option<u64> {
        self.address
    }

    pub fn set_address(&mut self, address: Option<u64>) {
        self.address = address;
    }

    pub fn index(&self) -> PciBarIndex {
        self.index
    }

    pub fn region_type(&self) -> PciBarType {
        self.region_type
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PciBarIndex(usize);

impl PciBarIndex {
    pub fn into_inner(self) -> usize {
        self.0
    }

    pub fn max() -> Self {
        Self(NUM_BAR_REGS - 1)
    }
}

impl ops::Add<Self> for PciBarIndex {
    type Output = Result<Self>;
    fn add(self, rhs: Self) -> Result<Self> {
        let val = self.0 + rhs.0;
        if val < NUM_BAR_REGS {
            Ok(Self(val))
        } else {
            Err(Error::InvalidBarIndex { index: val })
        }
    }
}

impl ops::Add<usize> for PciBarIndex {
    type Output = Result<Self>;

    fn add(self, rhs: usize) -> Result<Self> {
        let val = self.0.checked_add(rhs).ok_or(Error::InvalidBarIndex {
            index: self.0 + rhs,
        })?;
        if val < NUM_BAR_REGS {
            Ok(Self(val))
        } else {
            Err(Error::InvalidBarIndex {
                index: self.0 + rhs,
            })
        }
    }
}

impl TryFrom<usize> for PciBarIndex {
    type Error = Error;

    fn try_from(index: usize) -> Result<Self> {
        if index < NUM_BAR_REGS {
            Ok(Self(index))
        } else {
            Err(Error::InvalidBarIndex { index })
        }
    }
}
