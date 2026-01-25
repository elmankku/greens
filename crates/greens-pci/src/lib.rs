// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

#[cfg(all(not(feature = "std"), not(test)))]
extern crate core as std;

#[cfg(feature = "std")]
use std::error;
use std::fmt;
use std::result;

use intx::{PciInterruptLine, PciInterruptLineState};

pub mod bar;
pub mod bar_region;
pub mod capability;
pub mod config_handler;
pub mod configuration_space;
pub mod device;
pub mod function;
pub mod interrupt;
pub mod intx;
pub mod msi;
pub mod msix;
pub mod registers;
pub mod utils;

#[derive(Debug, PartialEq)]
pub enum Error {
    BarInUse { index: usize },
    BarNotFound { address: u64, size: u64 },
    BarRegionOverflow { address: u64, size: u64 },
    InvalidBarAlignment { address: u64, size: u64 },
    InvalidBarIndex { index: usize },
    InvalidBarSize { size: u64 },
    InvalidBarAddress { address: u64 },
    InvalidIoSize { size: usize },
    InvalidAccessAlignment { offset: u64, size: u64 },
    AccessBounds { offset: usize, size: usize },
    NoInterrupt,
    UnsupportedHeader { header_type: u8 },
    NotSupported,
    CapabilityNotFound { cap: u8 },
    ConfigurationSpaceBounds { limit: usize },
    DeviceAreaBounds { offset: usize, limit: usize },
    InvalidMsiVector { vector: u8 },
    InvalidMultipleMessageValue { value: u16 },
    MsiDisabled,
    NotBusMaster,
    InvalidFunction { function: usize },
    InvalidMsiXVector { vector: usize },
    InterruptTypeMismatch,
    InvalidMsiXTableOffset { offset: u64 },
    InvalidMsiXTableSize { size: usize },
    InvalidMsiXBarSize { size: u64 },
    VectorNotMasked { vector: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::Error::*;
        match self {
            BarInUse { index } => write!(f, "bar already in use: {index}"),
            BarNotFound { address, size } => {
                write!(f, "no bar found for {address} with size {size}")
            }
            BarRegionOverflow { address, size } => {
                write!(f, "bar at {address} with {size} would overflow")
            }
            InvalidBarAlignment { address, size } => {
                write!(f, "bar is not naturally aligned: {address} % {size} != 0")
            }
            InvalidBarIndex { index } => write!(f, "invalid bar index: {index}"),
            InvalidBarSize { size } => write!(f, "invalid bar size: {size}"),
            InvalidBarAddress { address } => write!(f, "invalid bar address: {address}"),
            InvalidIoSize { size } => write!(f, "invalid io size: {size}"),
            InvalidAccessAlignment { offset, size } => {
                write!(f, "offset `{offset}` not aligned to size `{size}`")
            }
            AccessBounds { offset, size } => write!(
                f,
                "access to offset `{offset}` with `{size}` bytes is out of bounds"
            ),
            NoInterrupt => write!(f, "no interrupt configured for the device"),
            UnsupportedHeader { header_type } => write!(f, "unsupported pci header: {header_type}"),
            NotSupported => write!(f, "functionality not supported"),
            CapabilityNotFound { cap } => write!(f, "pci capability with id {cap} not found"),
            ConfigurationSpaceBounds { limit } => {
                write!(f, "configuration space bounds exceeded: {limit}")
            }
            DeviceAreaBounds { offset, limit } => {
                write!(f, "device area bounds exceeded: {offset} > {limit}")
            }
            InvalidMsiVector { vector } => write!(f, "invalid msi vector: {vector}"),
            InvalidMultipleMessageValue { value } => {
                write!(f, "invalid msi multiple message value: {value}")
            }
            MsiDisabled => write!(f, "msi disabled for the device"),
            NotBusMaster => write!(f, "device is not bus master"),
            InvalidFunction { function } => write!(f, "invalid device function: {function}"),
            InvalidMsiXVector { vector } => write!(f, "invalid msi-x vector: {vector}"),
            InterruptTypeMismatch => write!(f, "attempting to send interrupt of wrong type"),
            InvalidMsiXTableOffset { offset } => write!(f, "invalid msix table offset: {offset}"),
            InvalidMsiXTableSize { size } => write!(f, "invalid msix table size: {size}"),
            InvalidMsiXBarSize { size } => write!(f, "invalid msix bar region size: {size}"),
            VectorNotMasked { vector } => write!(f, "invalid state: vector {vector} not masked"),
        }
    }
}

#[cfg(feature = "std")]
impl error::Error for Error {}

pub type Result<T> = result::Result<T, Error>;

#[derive(Debug, PartialEq)]
pub struct PciMsiMessage {
    pub address: u64,
    pub data: u32,
}

pub trait PciInterruptController {
    fn set_interrupt(&mut self, line: PciInterruptLine, state: PciInterruptLineState);

    fn send_msi(&mut self, message: PciMsiMessage);
}
