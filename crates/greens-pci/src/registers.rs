// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
#[cfg(feature = "pcie")]
pub const PCI_CONFIGURATION_SPACE_SIZE: usize = 4096;
#[cfg(not(feature = "pcie"))]
pub const PCI_CONFIGURATION_SPACE_SIZE: usize = 256;

pub const PCI_CONFIGURATION_SPACE_MAX_IO_SIZE: usize = 4;

pub const PCI_MAX_DEVICE_FUNCTIONS: usize = 7;

pub const PCI_TYPE0_NUM_BARS: usize = 6;
pub const PCI_TYPE1_NUM_BARS: usize = 2;
pub const NUM_BAR_REGS: usize = PCI_TYPE0_NUM_BARS;

pub const PCI_VENDOR_ID: usize = 0x0;
pub const PCI_DEVICE_ID: usize = 0x2;

pub const PCI_COMMAND: usize = 0x4;
pub const PCI_COMMAND_IO_SPACE_MASK: u16 = 1 << 0;
pub const PCI_COMMAND_MEM_SPACE_MASK: u16 = 1 << 1;
pub const PCI_COMMAND_BUS_MASTER_MASK: u16 = 1 << 2;
pub const PCI_COMMAND_INTERRUPT_DISABLE_MASK: u16 = 1 << 10;

pub const PCI_STATUS: usize = 0x6;
pub const PCI_STATUS_INTERRUPT_STATUS_MASK: u16 = 1 << 3;
pub const PCI_STATUS_CAP_LIST_MASK: u16 = 1 << 4;

pub const PCI_REVISION_ID: usize = 0x8;
pub const PCI_CLASS_CODE_PI: usize = 0x9;
pub const PCI_CLASS_CODE_SUB: usize = 0xA;
pub const PCI_CLASS_CODE_BASE: usize = 0xB;

pub const PCI_CACHE_LINE_SIZE: usize = 0xC;
pub const PCI_HEADER_TYPE: usize = 0xE;
pub const PCI_HEADER_TYPE_MULTIFUNCTION: u8 = 1 << 7;

pub const PCI_BAR0: usize = 0x10;
pub const PCI_BAR1: usize = 0x14;
pub const PCI_BAR2: usize = 0x18;
pub const PCI_BAR3: usize = 0x1C;
pub const PCI_BAR4: usize = 0x20;
pub const PCI_BAR5: usize = 0x24;

pub const PCI_BAR_IO_MIN_SIZE: u64 = 16;
// PCI spec states that max 256 locations per I/O BAR
pub const PCI_BAR_IO_MAX_SIZE: u64 = 256;
pub const PCI_BAR_IO_INDICATOR_MASK: u32 = 0x01;
pub const PCI_BAR_IO_BASE_ADDRESS_MASK: u32 = 0xFFFF_FFFC;

pub const PCI_BAR_MEM_MIN_SIZE: u64 = 128;
pub const PCI_BAR_MEM_32_MAX_SIZE: u64 = 2 << 30;
pub const PCI_BAR_MEM_64_MAX_SIZE: u64 = 256 << 30;
pub const PCI_BAR_MEM_32_BASE_ADDRESS_MASK: u32 = 0xFFFF_FFF0;
pub const PCI_BAR_MEM_64_BASE_ADDRESS_MASK: u32 = 0xFFFF_FFFF;
pub const PCI_BAR_MEM_32_INDICATOR_MASK: u32 = 0x00;
pub const PCI_BAR_MEM_64_INDICATOR_MASK: u32 = 0x04;
pub const PCI_BAR_MEM_PREFETCHABLE_MASK: u32 = 0x08;

pub const PCI_SUBSYSTEM_VENDOR_ID: usize = 0x2C;
pub const PCI_SUBSYSTEM_ID: usize = 0x2E;

pub const PCI_EXP_ROM_BAR: usize = 0x30;
pub const PCI_CAP_POINTER: usize = 0x34;
pub const PCI_CAP_POINTER_RSVD_MASK: u8 = 0x3;
// Offsets within CAP header
pub const PCI_CAP_HEADER_ID: usize = 0x0;
pub const PCI_CAP_HEADER_NEXT: usize = 0x1;
pub const PCI_CAP_DATA_START: usize = 0x2;

pub const PCI_INTERRUPT_LINE: usize = 0x3C;
pub const PCI_INTERRUPT_PIN: usize = 0x3D;

pub const PCI_HEADER_END: usize = 0x3F;
pub const PCI_DEVICE_SPECIFIC_START: usize = PCI_HEADER_END + 1;
pub const PCI_DEVICE_SPECIFIC_END: usize = 0xFF;

pub const PCI_EXTENDED_SPACE_START: usize = PCI_DEVICE_SPECIFIC_END + 1;
pub const PCI_EXTENDED_SPACE_END: usize = 0xFFF;
