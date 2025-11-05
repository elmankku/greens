// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::bar::{PciBar, PciBarIndex};
use greens_pci::capability::{PciCapOffset, PciCapability, PciCapabilityId};
use greens_pci::function::{PciFunction, PciFunctionConfigAccessor};
use greens_pci::utils::range_overlaps;
use greens_pci::utils::register_block::set_dword;

use crate::pci_cap::{virtio_cap_len, VirtioPciCap, VirtioPciCapType};
use crate::pci_cap::{REG_BAR, REG_LENGTH, REG_OFFSET, VIRTIO_CAP_SIZE};

pub(crate) const REG_PCI_CFG_DATA: usize = 14;
pub(crate) const VIRTIO_PCI_CFG_CAP_SIZE: usize = VIRTIO_CAP_SIZE + 4;

pub struct VirtioPciCfgCap {
    cap: VirtioPciCap,
    pci_cfg_data: [u8; 4],
}

impl VirtioPciCfgCap {
    pub fn new() -> Self {
        Self {
            cap: VirtioPciCap::new(
                Some(virtio_cap_len(VIRTIO_PCI_CFG_CAP_SIZE)),
                VirtioPciCapType::PciCfg,
                0,
                0,
                0,
                0,
            ),
            pci_cfg_data: [0xFFu8; 4],
        }
    }
}

impl Default for VirtioPciCfgCap {
    fn default() -> Self {
        Self::new()
    }
}

pub trait VirtioPciCfgHandler: PciFunction + PciFunctionConfigAccessor {
    fn pci_cfg_cap(&self) -> VirtioPciCfg;
}

impl PciCapability for VirtioPciCfgCap {
    fn id(&self) -> PciCapabilityId {
        self.cap.id()
    }

    fn size(&self) -> usize {
        self.cap.size() + self.pci_cfg_data.len()
    }

    fn registers(&self, registers: &mut [u8]) {
        self.cap.registers(&mut registers[0..VIRTIO_CAP_SIZE]);
        registers[VIRTIO_CAP_SIZE..].copy_from_slice(&self.pci_cfg_data)
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        writable_bits.fill(0);
        // cap.bar RW
        writable_bits[REG_BAR] = 0xFF;
        // cap.length RW
        set_dword(writable_bits, REG_LENGTH, 0xFFFF_FFFF);
        // cap.offset RW
        set_dword(writable_bits, REG_OFFSET, 0xFFFF_FFFF);
        // pci_cfg_data RW
        set_dword(writable_bits, REG_PCI_CFG_DATA, 0xFFFF_FFFF);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VirtioPciCfg {
    offset: PciCapOffset,
}

impl VirtioPciCfg {
    pub fn new(offset: PciCapOffset) -> Self {
        Self { offset }
    }

    pub fn prepare_read(
        &self,
        function: &mut dyn VirtioPciCfgHandler,
        offset: usize,
        size: usize,
    ) -> bool {
        if !self.should_handle_access(offset, size) {
            return false;
        }

        // Default to invalid data
        set_pci_cfg_data_invalid(function);

        let Some(bar) = find_bar(function) else {
            // Driver supplied invalid BAR.
            return true;
        };

        let mut data = [0xFFu8; 4];
        let (read_offset, read_length) = access_params(function);

        let read_length = read_length as usize;
        if read_length > data.len() {
            // Driver supplied invalid length.
            return true;
        }

        let ret = match bar.is_mem() {
            true => function.read_mmio(read_offset as u64, &mut data[..read_length]),
            false => {
                if read_offset > u32::from(u16::MAX) {
                    // Driver supplied invalid offset.
                    return true;
                }
                function.read_pio(read_offset as u16, &mut data[..read_length])
            }
        };

        match ret {
            // Success, set valid data for reading
            Ok(_) => update_pci_cfg_data(function, &data),
            // Accessing BAR failed, read returns invalid data.
            Err(_) => set_pci_cfg_data_invalid(function),
        }

        true
    }

    pub fn process_write(
        &self,
        function: &mut dyn VirtioPciCfgHandler,
        offset: usize,
        size: usize,
    ) -> bool {
        if !self.should_handle_access(offset, size) {
            return false;
        }

        let Some(bar) = find_bar(function) else {
            // Driver supplied invalid BAR.
            return true;
        };

        let (read_offset, read_length) = access_params(function);

        let mut data = [0x00u8; 4];
        get_pci_cfg_data(function, &mut data);

        let read_length = read_length as usize;
        if read_length > data.len() {
            // Driver supplied invalid length.
            return true;
        }

        _ = match bar.is_mem() {
            true => function.write_mmio(read_offset as u64, &data[..read_length]),
            false => {
                if read_offset > u32::from(u16::MAX) {
                    // Driver supplied invalid offset.
                    return true;
                }
                function.write_pio(read_offset as u16, &data[..read_length])
            }
        };

        true
    }

    fn should_handle_access(&self, offset: usize, size: usize) -> bool {
        range_overlaps(offset, size, self.pci_cfg_data_offset(), 4)
    }

    pub fn pci_cfg_data_offset(&self) -> usize {
        self.offset + REG_PCI_CFG_DATA
    }
}

fn find_bar(function: &dyn VirtioPciCfgHandler) -> Option<PciBar> {
    let bar = function
        .config()
        .read_byte(function.pci_cfg_cap().offset + REG_BAR);

    let Ok(bar) = PciBarIndex::try_from(bar as usize) else {
        // Driver supplied invalid index.
        return None;
    };

    function.get_bar(bar)
}

fn access_params(function: &dyn VirtioPciCfgHandler) -> (u32, u32) {
    let cap_offset = function.pci_cfg_cap().offset;

    let config = function.config();
    let offset = config.read_dword(cap_offset + REG_OFFSET);

    let length = config.read_dword(cap_offset + REG_LENGTH);

    (offset, length)
}

pub fn pci_cfg_data_offset(function: &dyn VirtioPciCfgHandler) -> usize {
    function.pci_cfg_cap().offset + REG_PCI_CFG_DATA
}

fn get_pci_cfg_data(function: &dyn VirtioPciCfgHandler, data: &mut [u8]) {
    function.config().read(pci_cfg_data_offset(function), data)
}

fn update_pci_cfg_data(function: &mut dyn VirtioPciCfgHandler, data: &[u8]) {
    let pci_cfg_data = pci_cfg_data_offset(function);

    function.config_mut().write(pci_cfg_data, data)
}

fn set_pci_cfg_data_invalid(function: &mut dyn VirtioPciCfgHandler) {
    update_pci_cfg_data(function, &0xFFFF_FFFFu32.to_ne_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pci_cap::tests::{check_cap, check_cap_offs_len, check_cap_ro_fields};
    use greens_pci::bar::{PciBar, PciBarPrefetchable::NotPrefetchable, PciBarType};
    use greens_pci::configuration_space::PciConfigurationSpace;
    use greens_pci::function::PciFunction;
    use greens_pci::registers::{
        PCI_BAR_IO_MIN_SIZE, PCI_BAR_MEM_MIN_SIZE, PCI_COMMAND, PCI_COMMAND_IO_SPACE_MASK,
        PCI_COMMAND_MEM_SPACE_MASK,
    };

    struct TestFunction {
        config: PciConfigurationSpace,
        bar: [u8; 20],
        cap: VirtioPciCfg,
    }

    impl PciFunction for TestFunction {
        fn read_config(&mut self, offset: usize, data: &mut [u8]) -> greens_pci::Result<()> {
            self.pci_cfg_cap().prepare_read(self, offset, data.len());
            self.config.read_checked(offset, data)
        }

        fn write_config(&mut self, offset: usize, data: &[u8]) -> greens_pci::Result<()> {
            self.config.write_checked(offset, data)?;
            self.pci_cfg_cap().process_write(self, offset, data.len());
            Ok(())
        }

        fn get_bar(&self, index: PciBarIndex) -> Option<greens_pci::bar::PciBar> {
            self.config.get_bar(index)
        }

        fn read_bar(
            &mut self,
            bar_index: PciBarIndex,
            offset: u64,
            data: &mut [u8],
        ) -> greens_pci::Result<()> {
            if bar_index.into_inner() < 2 {
                let offset = offset as usize;
                data.copy_from_slice(&self.bar[offset..offset + data.len()]);
                Ok(())
            } else {
                Err(greens_pci::Error::NotSupported)
            }
        }

        fn write_bar(
            &mut self,
            bar_index: PciBarIndex,
            offset: u64,
            data: &[u8],
        ) -> greens_pci::Result<()> {
            if bar_index.into_inner() < 2 {
                let offset = offset as usize;
                self.bar[offset..offset + data.len()].copy_from_slice(data);
                Ok(())
            } else {
                Err(greens_pci::Error::NotSupported)
            }
        }
    }

    impl PciFunctionConfigAccessor for TestFunction {
        fn config(&self) -> &PciConfigurationSpace {
            &self.config
        }

        fn config_mut(&mut self) -> &mut PciConfigurationSpace {
            &mut self.config
        }
    }

    impl VirtioPciCfgHandler for TestFunction {
        fn pci_cfg_cap(&self) -> VirtioPciCfg {
            self.cap
        }
    }

    impl TestFunction {
        fn new() -> Self {
            let mut config = PciConfigurationSpace::new();

            let offset = config
                .add_capability(&VirtioPciCfgCap::new())
                .expect("adding cap failed");

            let mem = PciBar::new(
                None,
                PCI_BAR_MEM_MIN_SIZE,
                PciBarIndex::try_from(0).unwrap(),
                PciBarType::Memory32Bit(NotPrefetchable),
            );
            config.add_bar(mem).expect("adding mem bar failed");

            let pio = PciBar::new(
                None,
                PCI_BAR_IO_MIN_SIZE,
                PciBarIndex::try_from(1).unwrap(),
                PciBarType::Io,
            );
            config.add_bar(pio).expect("adding io bar failed");

            config.set_word(
                PCI_COMMAND,
                PCI_COMMAND_MEM_SPACE_MASK | PCI_COMMAND_IO_SPACE_MASK,
            );

            let mut bar = [0x00u8; 20];
            bar[8..16].copy_from_slice(&0x11223344_55667788u64.to_ne_bytes());

            let cap = VirtioPciCfg::new(offset);

            Self { cap, config, bar }
        }
    }

    #[test]
    fn test_cap_init() {
        let mut config = PciConfigurationSpace::new();

        config
            .add_capability(&VirtioPciCfgCap::new())
            .expect("adding cap failed");

        check_cap(
            &config,
            VIRTIO_PCI_CFG_CAP_SIZE,
            VirtioPciCapType::PciCfg,
            0,
        );
        check_cap_offs_len(&config, 0, 0);

        // bar, offset, length and data are writable
        let mut rw_fields = [0x00u8; VIRTIO_PCI_CFG_CAP_SIZE];
        rw_fields[REG_BAR] = 0xFF;
        rw_fields[REG_OFFSET..REG_PCI_CFG_DATA + 4].fill(0xFF);

        check_cap_ro_fields(&mut config, &rw_fields);
    }

    fn set_access(fun: &mut TestFunction, bar: u8, offset: u32, length: u32) {
        let (_, cap) = fun.config.capability_iter().last().unwrap();

        fun.config.write_byte(cap + REG_BAR, bar);
        fun.config.write_dword(cap + REG_OFFSET, offset);
        fun.config.write_dword(cap + REG_LENGTH, length);
    }

    fn read_data(fun: &mut TestFunction, data: &mut [u8]) {
        let (_, cap) = fun.config.capability_iter().last().unwrap();
        fun.read_config(cap + REG_PCI_CFG_DATA, data).unwrap();
    }

    #[test]
    fn test_pci_cfg_read_invalid_bar() {
        let mut fun = TestFunction::new();
        set_access(&mut fun, 3, 8, 4);

        let mut data = [0x0u8; 4];
        read_data(&mut fun, &mut data);

        assert_eq!(u32::from_ne_bytes(data), 0xFFFF_FFFF);
    }

    #[test]
    fn test_pci_cfg_read_invalid_length() {
        let mut fun = TestFunction::new();
        set_access(&mut fun, 0, 8, 0);

        let mut data = [0x0u8; 4];
        read_data(&mut fun, &mut data);

        assert_eq!(u32::from_ne_bytes(data), 0xFFFF_FFFF);
    }

    #[test]
    fn test_pci_cfg_read() {
        let mut fun = TestFunction::new();
        set_access(&mut fun, 0, 8, 4);

        let mut data = [0x0u8; 4];
        read_data(&mut fun, &mut data);

        assert_eq!(u32::from_ne_bytes(data), 0x5566_7788);

        set_access(&mut fun, 1, 12, 4);
        read_data(&mut fun, &mut data);
        assert_eq!(u32::from_ne_bytes(data), 0x1122_3344);
    }

    fn write_data(fun: &mut TestFunction, data: &[u8]) {
        let (_, cap) = fun.config.capability_iter().last().unwrap();
        fun.write_config(cap + REG_PCI_CFG_DATA, data).unwrap();
    }

    #[test]
    fn test_pci_cfg_write_invalid_bar() {
        let mut fun = TestFunction::new();
        set_access(&mut fun, 3, 4, 4);

        let data = 0x1122_3344u32.to_ne_bytes();
        write_data(&mut fun, &data);

        assert_ne!(fun.bar[4..8], data);
    }

    #[test]
    fn test_pci_cfg_write_invalid_length() {
        let mut fun = TestFunction::new();
        set_access(&mut fun, 0, 4, 0);

        let data = 0x1122_3344u32.to_ne_bytes();
        write_data(&mut fun, &data);

        assert_ne!(fun.bar[4..8], data);
    }

    #[test]
    fn test_pci_cfg_write() {
        let mut fun = TestFunction::new();
        set_access(&mut fun, 0, 4, 4);

        let data = 0x1122_3344u32.to_ne_bytes();
        write_data(&mut fun, &data);

        assert_eq!(fun.bar[4..8], data);
    }
}
