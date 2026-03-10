// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use greens_pci::bar::PciBarIndex;
use greens_pci::bar_region::{PciBarRegion, PciBarRegionHandler, PciBarRegionInfo};
use greens_pci::capability::{PciCapability, PciCapabilityId};
use greens_pci::utils::{
    EndianSwapSize, from_little_endian, read, set, set_byte, set_word, to_little_endian, write,
};
use greens_pci::{Error, Result};
use virtio_queue::{Queue, QueueT};

use crate::pci::VirtioPciDevice;
use crate::pci_cap::{VirtioPciCap, VirtioPciCapType};

pub struct VirtioPciCommonCfgCap(VirtioPciCap);

impl VirtioPciCommonCfgCap {
    pub fn new(bar: u8, offset: u32, length: u32) -> Self {
        Self(VirtioPciCap::new(
            None,
            VirtioPciCapType::CommonCfg,
            bar,
            0,
            offset,
            length,
        ))
    }
}

impl PciCapability for VirtioPciCommonCfgCap {
    fn id(&self) -> PciCapabilityId {
        self.0.id()
    }

    fn size(&self) -> usize {
        self.0.size()
    }

    fn registers(&self, registers: &mut [u8]) {
        self.0.registers(registers)
    }

    fn writable_bits(&self, writable_bits: &mut [u8]) {
        self.0.writable_bits(writable_bits)
    }
}

// Affecting the whole device.
const REG_DEVICE_FEATURE_SELECT: usize = 0;
const REG_DEVICE_FEATURE: usize = 4;
const REG_DRIVER_FEATURE_SELECT: usize = 8;
const REG_DRIVER_FEATURE: usize = 12;
const REG_CONFIG_MSIX_VECTOR: usize = 16;
const REG_NUM_QUEUES: usize = 18;
const REG_DEVICE_STATUS: usize = 20;
const REG_CONFIG_GENERATION: usize = 21;

// Affecting a specific virtqueue.
const REG_QUEUE_SELECT: usize = 22;
const REG_QUEUE_SIZE: usize = 24;
const REG_QUEUE_MSIX_VECTOR: usize = 26;
const REG_QUEUE_ENABLE: usize = 28;
const REG_QUEUE_NOTIFY_OFF: usize = 30;
const REG_QUEUE_DESC: usize = 32;
const REG_QUEUE_DRIVER: usize = 40;
const REG_QUEUE_DEVICE: usize = 48;
// New in 1.2.
const REG_QUEUE_NOTIF_CONFIG_DATA: usize = 56;
const REG_QUEUE_RESET: usize = 58;

// Affecting the administration virtqueue. New in 1.3.
const REG_ADMIN_QUEUE_INDEX: usize = 60;
const REG_ADMIN_QUEUE_NUM: usize = 62;

const VIRTIO_COMMON_CFG_SIZE: usize = 64;

pub const MSIX_VECTOR_UNMAPPED: u16 = 0xFFFF;

#[derive(Debug, Clone)]
pub struct VirtioPciCommonCfg {
    info: PciBarRegionInfo,
    registers: [u8; VIRTIO_COMMON_CFG_SIZE],
    writable_bits: [u8; VIRTIO_COMMON_CFG_SIZE],
}

impl PciBarRegionHandler for VirtioPciCommonCfg {
    type Context<'a> = &'a mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue>;
    type R = ();

    fn read_bar(
        &mut self,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        self.prepare_read(offset, data.len(), *context)?;
        self.read_registers(offset, data)?;
        to_little_endian(data, EndianSwapSize::Qword)
    }

    fn write_bar(
        &mut self,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R> {
        let size = data.len();

        if size > EndianSwapSize::Qword as usize {
            return Err(Error::InvalidIoSize { size });
        }
        let mut input = [0x00u8; EndianSwapSize::Qword as usize];
        input[..size].copy_from_slice(data);
        from_little_endian(&mut input, EndianSwapSize::Qword)?;

        self.write_registers(offset, &input).or_else(|e| match e {
            Error::AccessBounds { .. } => Ok(()),
            _ => Err(e),
        })?;

        self.process_write(offset, size, *context)
    }
}

impl PciBarRegion for VirtioPciCommonCfg {
    fn info(&self) -> &PciBarRegionInfo {
        &self.info
    }
}

impl VirtioPciCommonCfg {
    pub fn new(bar: PciBarIndex, offset: u64, length: u64) -> Self {
        let mut writable_bits = [0x00u8; VIRTIO_COMMON_CFG_SIZE];

        writable_bits[..REG_DEVICE_FEATURE].fill(0xFF);
        writable_bits[REG_DRIVER_FEATURE_SELECT..REG_NUM_QUEUES].fill(0xFF);
        writable_bits[REG_DEVICE_STATUS] = 0xFF;

        writable_bits[REG_QUEUE_SELECT..REG_QUEUE_NOTIFY_OFF].fill(0xFF);
        writable_bits[REG_QUEUE_DESC..REG_QUEUE_NOTIF_CONFIG_DATA].fill(0xFF);
        writable_bits[REG_QUEUE_RESET..REG_ADMIN_QUEUE_INDEX].fill(0xFF);
        set_word(&mut writable_bits, REG_QUEUE_RESET, 0xFFFF).unwrap();

        Self {
            info: PciBarRegionInfo::new(bar, offset, length),
            registers: [0x00u8; VIRTIO_COMMON_CFG_SIZE],
            writable_bits,
        }
    }

    fn read_registers(&mut self, offset: u64, data: &mut [u8]) -> Result<()> {
        read(&self.registers, offset as usize, data)
    }

    fn write_registers(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        write(
            &mut self.registers,
            &self.writable_bits,
            offset as usize,
            data,
        )
    }

    fn prepare_read(
        &mut self,
        offset: u64,
        size: usize,
        device: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
    ) -> Result<()> {
        let offset = offset as usize;
        let Ok(offset) = Config::try_from(offset) else {
            // Invalid offset, set invalidate data
            self.set_config_data_invalid(offset, size);
            return Ok(());
        };

        match offset {
            Config::DevFeature => {
                let select = self.config_dword(Config::DevFeatureSel);
                let features = paged_features(device.device_features(), select);
                self.set_config_dword(offset, features);
            }
            Config::DrvFeature => {
                let select = self.config_dword(Config::DrvFeatureSel);
                let features = paged_features(device.driver_features(), select);
                self.set_config_dword(offset, features);
            }
            Config::CfgMsixVector => {
                self.set_config_word(offset, device.config_msix_vector());
            }
            Config::NumQueues => {
                self.set_config_word(offset, device.num_queues());
            }
            Config::DevStatus => self.set_config_byte(offset, device.device_status()),
            Config::CfgGeneration => self.set_config_byte(offset, device.config_generation()),
            Config::QueueSel => self.set_config_word(offset, device.queue_select()),
            Config::QueueSize => {
                self.set_config_word(offset, self.with_queue(device, |q| q.size()).unwrap_or(0))
            }
            Config::QueueMsixVector => self.set_config_word(
                offset,
                device.queue_msix_vector().unwrap_or(MSIX_VECTOR_UNMAPPED),
            ),
            Config::QueueEnable => self.set_config_word(
                offset,
                self.with_queue(device, |q| q.ready().into()).unwrap_or(0),
            ),
            Config::QueueNotifyOff => {
                let select = self.config_word(Config::QueueSel);
                self.set_config_word(offset, device.queue(select).map_or(0, |_| select));
            }
            Config::QueueDesc | Config::QueueDescHi => self.set_config_qword(
                Config::QueueDesc,
                self.with_queue(device, |q| q.desc_table()).unwrap_or(0),
            ),
            Config::QueueDrv | Config::QueueDrvHi => self.set_config_qword(
                Config::QueueDrv,
                self.with_queue(device, |q| q.avail_ring()).unwrap_or(0),
            ),
            Config::QueueDev | Config::QueueDevHi => self.set_config_qword(
                Config::QueueDev,
                self.with_queue(device, |q| q.used_ring()).unwrap_or(0),
            ),
            Config::QueueNotifyCfgData => todo!(),
            Config::QueueReset => todo!(),
            Config::AdmQueueIndex => todo!(),
            Config::AdmQueueNum => todo!(),
            // Nothing stored here.
            Config::DevFeatureSel | Config::DrvFeatureSel => (),
        };

        Ok(())
    }

    fn process_write(
        &mut self,
        offset: u64,
        size: usize,
        device: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
    ) -> Result<()> {
        let offset = offset as usize;
        let Ok(offset) = Config::try_from(offset) else {
            // Invalid offset, set invalidate data
            self.set_config_data_invalid(offset, size);
            return Ok(());
        };

        match offset {
            Config::DrvFeature => {
                let features = self.config_dword(offset);
                device.set_driver_features(self.config_dword(Config::DrvFeatureSel), features);
            }
            Config::CfgMsixVector => device.set_config_msix_vector(self.config_word(offset)),
            Config::DevStatus => device.ack_device_status(self.config_byte(offset)),
            Config::QueueSel => {
                device.set_queue_select(self.config_word(offset));
            }
            Config::QueueSize => {
                let size = self.config_word(offset);
                self.with_queue_mut(device, |q| q.set_size(size));
            }
            Config::QueueMsixVector => device.set_queue_msix_vector(self.config_word(offset)),
            Config::QueueEnable => {
                if self.config_word(offset) == 1 {
                    self.with_queue_mut(device, |q| q.set_ready(true))
                }
            }
            Config::QueueDesc => {
                let (low, high) = address_parts(self.config_qword(offset), size);
                self.with_queue_mut(device, |q| q.set_desc_table_address(low, high))
            }
            Config::QueueDescHi => {
                let high = Some(self.config_dword(offset));
                self.with_queue_mut(device, |q| q.set_desc_table_address(None, high))
            }
            Config::QueueDrv => {
                let (low, high) = address_parts(self.config_qword(offset), size);
                self.with_queue_mut(device, |q| q.set_avail_ring_address(low, high))
            }
            Config::QueueDrvHi => {
                let high = Some(self.config_dword(offset));
                self.with_queue_mut(device, |q| q.set_avail_ring_address(None, high));
            }
            Config::QueueDev => {
                let (low, high) = address_parts(self.config_qword(offset), size);
                self.with_queue_mut(device, |q| q.set_used_ring_address(low, high))
            }
            Config::QueueDevHi => {
                let high = Some(self.config_dword(offset));
                self.with_queue_mut(device, |q| q.set_used_ring_address(None, high));
            }
            Config::QueueReset => {
                if self.config_word(offset) == 1 {
                    self.with_queue_mut(device, |q| q.reset());
                    self.set_config_word(offset, 0);
                }
            }
            // Do nothing - stored only to registers.
            Config::DevFeatureSel | Config::DrvFeatureSel => (),
            // Read-only for the driver.
            Config::DevFeature
            | Config::NumQueues
            | Config::CfgGeneration
            | Config::QueueNotifyOff
            | Config::QueueNotifyCfgData
            | Config::AdmQueueIndex
            | Config::AdmQueueNum => (),
        };

        Ok(())
    }

    pub fn config_msix_vector(self) -> u16 {
        self.config_word(Config::CfgMsixVector)
    }

    pub fn size() -> usize {
        VIRTIO_COMMON_CFG_SIZE
    }

    fn config_byte(&self, config: Config) -> u8 {
        let mut val = [0u8];
        self.config(config, &mut val);
        val[0]
    }

    fn config_word(&self, config: Config) -> u16 {
        let mut val = [0u8; 2];
        self.config(config, &mut val);
        u16::from_ne_bytes(val)
    }

    fn config_dword(&self, config: Config) -> u32 {
        let mut val = [0u8; 4];
        self.config(config, &mut val);
        u32::from_ne_bytes(val)
    }

    fn config_qword(&self, config: Config) -> u64 {
        let mut val = [0u8; 8];
        self.config(config, &mut val);
        u64::from_ne_bytes(val)
    }

    fn config(&self, config: Config, data: &mut [u8]) {
        assert_eq!(
            config.size(),
            data.len(),
            "BUG: read size does not match register size"
        );
        read(&self.registers, config.value(), data).expect("BUG: failed to read register")
    }

    fn set_config_byte(&mut self, config: Config, val: u8) {
        self.set_config(config, &val.to_ne_bytes());
    }

    fn set_config_word(&mut self, config: Config, val: u16) {
        self.set_config(config, &val.to_ne_bytes());
    }

    fn set_config_dword(&mut self, config: Config, val: u32) {
        self.set_config(config, &val.to_ne_bytes());
    }

    fn set_config_qword(&mut self, config: Config, val: u64) {
        self.set_config(config, &val.to_ne_bytes());
    }

    fn set_config(&mut self, config: Config, data: &[u8]) {
        assert_eq!(
            config.size(),
            data.len(),
            "BUG: set size does not match register size"
        );
        set(&mut self.registers, config.value(), data).expect("BUG: failed to update register")
    }

    fn set_config_data_invalid(&mut self, offset: usize, size: usize) {
        for i in offset..offset + size {
            let Ok(()) = set_byte(&mut self.registers, i, 0) else {
                return;
            };
        }
    }

    fn with_queue<U, F>(
        &self,
        device: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
        f: F,
    ) -> Option<U>
    where
        F: FnOnce(&Queue) -> U,
    {
        device.queue(device.queue_select()).map(f)
    }

    fn with_queue_mut<F>(
        &mut self,
        device: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue>,
        f: F,
    ) where
        F: FnOnce(&mut Queue),
    {
        if let Some(queue) = device.queue_mut(self.config_word(Config::QueueSel)) {
            f(queue)
        }
    }
}

fn address_parts(addr: u64, io_size: usize) -> (Option<u32>, Option<u32>) {
    let low = Some(addr as u32);
    let high = if io_size > 4 {
        Some((addr >> 32) as u32)
    } else {
        None
    };
    (low, high)
}

fn paged_features(features: u64, selection: u32) -> u32 {
    if selection < 2 {
        (features >> (selection * 32)) as u32
    } else {
        0
    }
}

#[doc(hidden)]
macro_rules! __check_register_types {
    (u8) => {};
    (u16) => {};
    (u32) => {};
    (u64) => {};
    ($other:ident) => {
        compile_error!(concat!(
            "Invalid type '",
            stringify!($other),
            "'. Only u8, u16, u32, and u64 are allowed."
        ));
    };
}

macro_rules! define_register_enum {
    (
        $offset_enum:ident,
        {
            $( $variant:ident is $ty:ident at $offset:expr ),* $(,)?
        }
    ) => {
        $(__check_register_types!($ty);)*

        #[derive(Debug, PartialEq, Eq, Clone, Copy)]
        #[repr(usize)]
        pub enum $offset_enum {
            $($variant = $offset,)*
        }

        impl $offset_enum {
            pub fn value(&self) -> usize {
                *self as usize
            }

            pub fn size(&self) -> usize {
                match *self {
                    $($offset_enum::$variant => core::mem::size_of::<$ty>(),)*
                }
            }
        }

        // For const bridge
        #[allow(non_upper_case_globals)]
        impl TryFrom<usize> for $offset_enum {
            type Error = ();

            fn try_from(value: usize) -> core::result::Result<Self, Self::Error> {
                $(const $variant: usize = $offset as usize;)*

                match value {
                    $($variant => Ok($offset_enum::$variant),)*
                    _ => Err(())
                }
            }
        }
    };
}

define_register_enum!(
Config,
{
    DevFeatureSel is u32 at REG_DEVICE_FEATURE_SELECT,
    DevFeature is u32 at REG_DEVICE_FEATURE,
    DrvFeatureSel is u32 at REG_DRIVER_FEATURE_SELECT,
    DrvFeature is u32 at REG_DRIVER_FEATURE,
    CfgMsixVector is u16 at REG_CONFIG_MSIX_VECTOR,
    NumQueues is u16 at REG_NUM_QUEUES,
    DevStatus is u8 at REG_DEVICE_STATUS,
    CfgGeneration is u8 at REG_CONFIG_GENERATION,
    QueueSel is u16 at REG_QUEUE_SELECT,
    QueueSize is u16 at REG_QUEUE_SIZE,
    QueueMsixVector is u16 at REG_QUEUE_MSIX_VECTOR,
    QueueEnable is u16 at REG_QUEUE_ENABLE,
    QueueNotifyOff is u16 at REG_QUEUE_NOTIFY_OFF,
    QueueDesc is u64 at REG_QUEUE_DESC,
    QueueDescHi is u32 at REG_QUEUE_DESC + 4,
    QueueDrv is u64 at REG_QUEUE_DRIVER,
    QueueDrvHi is u32 at REG_QUEUE_DRIVER + 4,
    QueueDev is u64 at REG_QUEUE_DEVICE,
    QueueDevHi is u32 at REG_QUEUE_DEVICE + 4,
    QueueNotifyCfgData is u16 at REG_QUEUE_NOTIF_CONFIG_DATA,
    QueueReset is u16 at REG_QUEUE_RESET,
    AdmQueueIndex is u16 at REG_ADMIN_QUEUE_INDEX,
    AdmQueueNum is u16 at REG_ADMIN_QUEUE_NUM,
});

#[cfg(test)]
pub mod tests {
    use std::borrow::{Borrow, BorrowMut};

    use greens_pci::PciMsiMessage;
    use greens_pci::configuration_space::PciConfigurationSpace;
    use virtio_device::{VirtioConfig, VirtioDeviceActions, VirtioDeviceType, WithDriverSelect};

    use super::*;
    use crate::pci_cap::VIRTIO_CAP_SIZE;
    use crate::pci_cap::tests::{check_cap, check_cap_offs_len};
    use crate::pci_isr_cfg::VirtioPciIsrState;

    #[test]
    fn test_common_cfg_cap() {
        let mut config = PciConfigurationSpace::new();

        let bar = 1;
        let offs = 0x1000;
        let len = 0x2000;

        config
            .add_capability(&VirtioPciCommonCfgCap::new(bar, offs, len))
            .expect("adding cap failed");

        check_cap(&config, VIRTIO_CAP_SIZE, VirtioPciCapType::CommonCfg, bar);
        check_cap_offs_len(&config, offs, len);

        // all fields RO
        crate::pci_cap::tests::check_cap_ro_fields(&mut config, &[0x00u8; VIRTIO_CAP_SIZE])
    }

    define_register_enum!(Foo, {
        Bar is u64 at 0,
        Baz is u32 at 8,
        Qux is u16 at 10,
        Quux is u8 at 12,
        Corge is u8 at 13,
    });

    fn test_register(variant: Foo, offset: usize, size: usize) {
        assert_eq!(variant.value(), offset);
        assert_eq!(variant.size(), size);
    }

    #[test]
    fn test_register_macro() {
        test_register(Foo::Bar, 0, 8);
        test_register(Foo::Baz, 8, 4);
        test_register(Foo::Qux, 10, 2);
        test_register(Foo::Quux, 12, 1);
        test_register(Foo::Corge, 13, 1);
    }

    pub(crate) struct TestDevice {
        pub cfg: VirtioConfig<Queue>,
        pub config_msix_vector: u16,
        pub queue_msix_vector: Vec<u16>,
    }

    impl VirtioPciDevice for TestDevice {
        fn set_config_msix_vector(&mut self, vector: u16) {
            self.config_msix_vector = vector;
        }

        fn config_msix_vector(&self) -> u16 {
            self.config_msix_vector
        }

        fn set_queue_msix_vector(&mut self, vector: u16) {
            let index = usize::from(self.queue_select());
            if let Some(v) = self.queue_msix_vector.get_mut(index) {
                *v = vector
            }
        }

        fn queue_msix_vector(&self) -> Option<u16> {
            let index = usize::from(self.queue_select());
            self.queue_msix_vector.get(index).cloned()
        }

        fn set_notification_info(
            &mut self,
            _notify_cfg_info: crate::pci_notify_cfg::VirtioPciNotifyCfgInfo,
        ) {
            todo!()
        }

        fn set_msi_message(&mut self, _vector: u16, _msg: PciMsiMessage) {
            todo!()
        }

        fn queue_notify(&mut self, _data: u32) {
            todo!()
        }
    }

    impl VirtioPciIsrState for TestDevice {
        fn read_and_clear_isr(&mut self) -> u8 {
            todo!()
        }
    }

    impl TestDevice {
        pub fn new() -> Self {
            let features = (0xBAAD << 32) | 0xABBA;
            let queue = Queue::new(256).unwrap();
            let cfg_space = vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
            let cfg = VirtioConfig::new(features, vec![queue], cfg_space);
            Self {
                cfg,
                config_msix_vector: 0,
                queue_msix_vector: vec![MSIX_VECTOR_UNMAPPED],
            }
        }
    }

    impl VirtioDeviceType for TestDevice {
        fn device_type(&self) -> u32 {
            10
        }
    }

    impl Borrow<VirtioConfig<Queue>> for TestDevice {
        fn borrow(&self) -> &VirtioConfig<Queue> {
            &self.cfg
        }
    }

    impl BorrowMut<VirtioConfig<Queue>> for TestDevice {
        fn borrow_mut(&mut self) -> &mut VirtioConfig<Queue> {
            &mut self.cfg
        }
    }

    impl VirtioDeviceActions for TestDevice {
        type E = greens_pci::Error;

        fn activate(&mut self) -> std::result::Result<(), Self::E> {
            // FIXME
            Ok(())
        }

        fn reset(&mut self) -> std::result::Result<(), Self::E> {
            // FIXME
            Ok(())
        }
    }

    fn cfg_w(cfgdev: &mut (VirtioPciCommonCfg, TestDevice), offset: Config, data: &[u8]) {
        let (cfg, dev) = cfgdev;
        let mut dev: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue> = dev;
        cfg.write_bar(offset.value() as u64, &data, &mut dev)
            .expect("write")
    }

    fn cfg_r(cfgdev: &mut (VirtioPciCommonCfg, TestDevice), offset: Config, data: &mut [u8]) {
        let (cfg, dev) = cfgdev;
        let mut dev: &mut dyn VirtioPciDevice<E = greens_pci::Error, Q = Queue> = dev;
        cfg.read_bar(offset.value() as u64, data, &mut dev)
            .expect("read")
    }

    // TODO: add tests for read/write
    #[test]
    fn test_device_features() {
        let mut cfgdev = (
            VirtioPciCommonCfg::new(PciBarIndex::default(), 0, VirtioPciCommonCfg::size() as u64),
            TestDevice::new(),
        );

        let mut read = [0u8; 4];

        // dev page 0
        cfg_w(&mut cfgdev, Config::DevFeatureSel, &0u32.to_ne_bytes());
        cfg_r(&mut cfgdev, Config::DevFeature, &mut read);
        assert_eq!(u32::from_le_bytes(read), 0xABBA);

        // dev page 1
        cfg_w(&mut cfgdev, Config::DevFeatureSel, &1u32.to_ne_bytes());
        cfg_r(&mut cfgdev, Config::DevFeature, &mut read);
        assert_eq!(u32::from_le_bytes(read), 0xBAAD);
    }
}
