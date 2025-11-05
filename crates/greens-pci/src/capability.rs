// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PciCapabilityId {
    NullCap = 0x00,
    PciPMInterface = 0x01,
    Agp = 0x02,
    Vpd = 0x03,
    SlotId = 0x04,
    Msi = 0x05,
    CompactPciHotSwap = 0x06,
    PciX = 0x07,
    HyperTransport = 0x08,
    VendorSpecific = 0x09,
    DebugPort = 0x0A,
    CompactPciResCtrl = 0x0B,
    PciHopPlug = 0x0C,
    PciBridgeVendorId = 0x0D,
    Agp8x = 0x0E,
    SecureDevice = 0x0F,
    PciExpress = 0x10,
    MsiX = 0x11,
    SerialAtaConfig = 0x12,
    AdvancedFeatures = 0x13,
    EnhancedAllocation = 0x14,
    FlatteningPortalBridge = 0x15,
    Others = 0xFF,
}

impl From<PciCapabilityId> for u8 {
    fn from(val: PciCapabilityId) -> Self {
        val as u8
    }
}

pub type PciCapOffset = usize;

/// The `PciCapability` trait allows inserting a capability the `PciConfigurationSpace`, and set
/// the capability data to the `PciConfigurationSpace`.
///
/// The capabilities must have PCI-SIG allocated ID and a size that fits to Device Specific area
/// (0x40-0xFF). The other functions allows the capability to set the register values and writable bits
/// bits to the `PciConfigurationSpace`.
pub trait PciCapability {
    /// The capability must return a valid PCI-SIG allocated ID.
    fn id(&self) -> PciCapabilityId;
    /// The size of the capability in bytes, excluding the standard 16-bit capability header. Size
    fn size(&self) -> usize;

    /// Set the values for the capability registers in the configuration space.
    ///
    /// The function receives a mutable slice to the register data, where the capability can set
    /// the data that is returned on reads.
    ///
    /// Although the size of the slice is `self.size()`, the capability should rely on `registers.len()`.
    fn registers(&self, registers: &mut [u8]);

    /// Set the writable bits for the capability registers in the configuration space.
    ///
    /// The function receives a mutable slice to the writable bits array, where the capability can set
    /// the bits that are writable. `1` indicates that the bit at the same offset in the registers
    /// is writable. Initially the registers are R/O.
    ///
    /// Although the size of the slice is `self.size()`, the capability should rely on `writable_bits.len()`.
    fn writable_bits(&self, writable_bits: &mut [u8]);
}
