// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::sync::{Arc, Mutex};

use crate::bindings::{
    driver_rpc_ack_mmio_finish, rpcmsg_receive, rpcmsg_t, vso_rpc, vso_rpc_id_vso_rpc_device,
    vso_rpc_init,
};
use greens_core::ioreq::*;
use greens_sys_linux::mmap::MemoryRegion;

use self::doorbell::MemoryRegionOffsetError;
pub use self::doorbell::{Doorbell, MmioDoorbell};

mod doorbell;

#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub enum RpcError {
    InvalidOffset(#[from] MemoryRegionOffsetError),
    #[error("rpc initialization failed: {0}")]
    Init(i32),
    #[error("invalid rpc request: {0}")]
    InvalidRequest(u32),
    #[error("invalid pci device: {0}")]
    InvalidPciDevice(u8),
    #[error("no available rpc requests")]
    RequestQueueEmpty,
    #[error("failed to complete io request: {0}")]
    IoRequestComplete(i32),
    #[error("invalid rpc message pointer")]
    InvalidMsgPointer,
    #[error("failed to lock mutex")]
    LockError,
}

#[derive(Debug)]
pub struct DriverRpc<T: MemoryRegion, U: Doorbell> {
    rpc: Arc<Mutex<vso_rpc>>,
    #[allow(dead_code)]
    io_mapping: T,
    #[allow(dead_code)]
    doorbell: U,
}

impl<T: MemoryRegion, U: Doorbell> DriverRpc<T, U> {
    pub fn new(io_mapping: T, doorbell: U) -> Result<DriverRpc<T, U>, RpcError> {
        let mut rpc = vso_rpc::default();

        let r = unsafe {
            vso_rpc_init(
                &mut rpc as *mut _,
                vso_rpc_id_vso_rpc_device,
                io_mapping.as_ptr() as *mut _,
                Some(U::ring),
                doorbell.cookie(),
            )
        };
        if r != 0 {
            return Err(RpcError::Init(r));
        }

        let rpc = Arc::new(Mutex::new(rpc));

        Ok(DriverRpc {
            rpc,
            io_mapping,
            doorbell,
        })
    }

    fn try_dequeue_ioreq(&self) -> Result<*mut rpcmsg_t, RpcError> {
        let mut rpc = self.rpc.lock().map_err(|_| RpcError::LockError)?;
        unsafe {
            let ioreq = rpcmsg_receive(&mut rpc.driver_rpc.request as *mut _);

            match ioreq.is_null() {
                true => Err(RpcError::RequestQueueEmpty),
                false => Ok(ioreq),
            }
        }
    }

    fn try_complete_ioreq(
        &self,
        msg: *mut rpcmsg_t,
        data: Option<[u8; 8]>,
    ) -> Result<(), RpcError> {
        let mut rpc = self.rpc.lock().map_err(|_| RpcError::LockError)?;
        unsafe {
            let data = match data {
                Some(bytes) => u64::from_ne_bytes(bytes),
                None => msg.as_ref().ok_or(RpcError::InvalidMsgPointer)?.mr2,
            };

            match driver_rpc_ack_mmio_finish(&mut *rpc, msg, data) {
                0 => Ok(()),
                v => Err(RpcError::IoRequestComplete(v)),
            }
        }
    }

    pub fn process_ioreq(
        &self,
        process_fn: &mut dyn FnMut(IoRequest) -> Option<[u8; 8]>,
    ) -> Result<(), RpcError> {
        let rpcmsg = self.try_dequeue_ioreq()?;
        let rpcmsg_ref = unsafe { rpcmsg.as_ref().ok_or(RpcError::InvalidMsgPointer) }?;

        self.try_complete_ioreq(rpcmsg, process_fn(IoRequest::from(rpcmsg_ref)))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::bindings::{
        IOBUF_NUM_PAGES, RPC_ADDRESS_SPACE_GLOBAL, RPC_MR0_MMIO_DIRECTION_READ,
        RPC_MR0_MMIO_DIRECTION_WRITE,
    };
    use crate::bindings::{
        driver_rpc_req_mmio_start, rpc_ioreq_address_space, rpc_ioreq_direction, rpc_ioreq_len,
        rpc_ioreq_slot,
    };
    use std::alloc::{self, Layout};

    #[derive(Debug, PartialEq)]
    pub(crate) struct TestMapping {
        buf: *mut u8,
        layout: Layout,
    }

    impl TestMapping {
        pub(crate) fn new(layout: Layout) -> Self {
            Self {
                buf: unsafe { alloc::alloc_zeroed(layout) },
                layout,
            }
        }
    }

    unsafe impl Send for TestMapping {}
    unsafe impl Sync for TestMapping {}

    unsafe impl MemoryRegion for TestMapping {
        fn as_ptr(&self) -> *mut u8 {
            self.buf
        }

        fn size(&self) -> usize {
            self.layout.size()
        }
    }

    impl Drop for TestMapping {
        fn drop(&mut self) {
            unsafe { alloc::dealloc(self.buf, self.layout) }
        }
    }

    type Mapping = TestMapping;
    type Doorbell = MmioDoorbell<TestMapping>;

    fn create_io_mapping() -> Mapping {
        let size = 4096 * IOBUF_NUM_PAGES as usize;
        let layout = Layout::from_size_align(size, 4096).expect("invalid layout");
        Mapping::new(layout)
    }

    fn create_doorbell() -> Doorbell {
        let layout: Layout = Layout::new::<u32>();
        Doorbell::new(Mapping::new(layout), None).unwrap()
    }

    fn create_rpc() -> DriverRpc<Mapping, Doorbell> {
        DriverRpc::new(create_io_mapping(), create_doorbell()).unwrap()
    }

    fn enqueue_ioreq(
        rpc: &DriverRpc<Mapping, Doorbell>,
        direction: u32,
        address_space: u32,
        slot: u32,
        address: u64,
        size: u8,
        data: u64,
    ) {
        let mut vso_rpc = rpc.rpc.lock().expect("lock error");
        let r = unsafe {
            driver_rpc_req_mmio_start(
                &mut *vso_rpc,
                direction,
                address_space,
                slot,
                address,
                size.into(),
                data,
            )
        };
        assert_eq!(r, 0);
    }

    fn dequeue_ioreq_reply(rpc: &DriverRpc<Mapping, Doorbell>) -> rpcmsg_t {
        let mut vso_rpc = rpc.rpc.lock().expect("lock error");
        unsafe {
            let ioreq = rpcmsg_receive(&mut vso_rpc.driver_rpc.response as *mut _);
            assert!(!ioreq.is_null());
            *ioreq.to_owned()
        }
    }

    fn assert_rpcmsg_eq(
        rpcmsg: &rpcmsg_t,
        direction: u32,
        address_space: u32,
        slot: u32,
        address: u64,
        size: u8,
        data: u64,
    ) {
        unsafe {
            assert_eq!(direction, rpc_ioreq_direction(rpcmsg.mr0));
            assert_eq!(address_space, rpc_ioreq_address_space(rpcmsg.mr0));
            assert_eq!(slot, rpc_ioreq_slot(rpcmsg.mr0));
            assert_eq!(address, rpcmsg.mr1);
            assert_eq!(size as u32, rpc_ioreq_len(rpcmsg.mr0));
            assert_eq!(data, rpcmsg.mr2);
        };
    }

    fn expect_no_ioreqs() -> impl FnMut(IoRequest) -> Option<[u8; 8]> {
        |_| panic!("Unexpected iorequests")
    }

    #[test]
    fn test_no_ioreqs() {
        let rpc = create_rpc();

        assert!(rpc.process_ioreq(&mut expect_no_ioreqs()).is_err());
    }

    #[test]
    fn test_process_mmio_write() {
        let rpc = create_rpc();
        let direction = RPC_MR0_MMIO_DIRECTION_WRITE;
        let address_space = RPC_ADDRESS_SPACE_GLOBAL;
        let slot = 3;
        let address = 0xabba_cafe;
        let size = 8;
        let data = 0x1234_5678;

        enqueue_ioreq(&rpc, direction, address_space, slot, address, size, data);

        let mut process_fn = |ioreq: IoRequest| {
            assert_eq!(ioreq.address_space, AddressSpace::Mmio);
            assert_eq!(ioreq.address, address);
            assert_eq!(ioreq.size, size);
            assert_eq!(
                ioreq.operation,
                IoOperation::Write {
                    data: data.to_ne_bytes()
                }
            );

            None
        };
        assert!(rpc.process_ioreq(&mut process_fn).is_ok());

        let rpcmsg = dequeue_ioreq_reply(&rpc);
        assert_rpcmsg_eq(&rpcmsg, direction, address_space, slot, address, size, data);

        assert!(rpc.process_ioreq(&mut expect_no_ioreqs()).is_err());
    }

    #[test]
    fn test_process_mmio_read() {
        let rpc = create_rpc();
        let direction = RPC_MR0_MMIO_DIRECTION_READ;
        let address_space = RPC_ADDRESS_SPACE_GLOBAL;
        let slot = 3;
        let address = 0xabba_cafe;
        let size = 8;
        let data = 0x1234_5678;
        let read_data = 0x1111_2222_3333_4444u64;

        enqueue_ioreq(&rpc, direction, address_space, slot, address, size, data);

        let mut process_fn = |ioreq: IoRequest| {
            assert_eq!(ioreq.address_space, AddressSpace::Mmio);
            assert_eq!(ioreq.address, address);
            assert_eq!(ioreq.size, size);
            assert_eq!(ioreq.operation, IoOperation::Read);

            Some(read_data.to_ne_bytes())
        };
        assert!(rpc.process_ioreq(&mut process_fn).is_ok());

        let rpcmsg = dequeue_ioreq_reply(&rpc);
        assert_rpcmsg_eq(
            &rpcmsg,
            direction,
            address_space,
            slot,
            address,
            size,
            read_data,
        );

        assert!(rpc.process_ioreq(&mut expect_no_ioreqs()).is_err());
    }

    #[test]
    fn test_process_pci_config_write() {
        let rpc = create_rpc();
        let direction = RPC_MR0_MMIO_DIRECTION_WRITE;
        let device = 7;
        let slot = 3;
        let address = 0xabba_cafe;
        let size = 4;
        let data = 0x1234_5678u64;

        enqueue_ioreq(&rpc, direction, device, slot, address, size, data);

        let mut process_fn = |ioreq: IoRequest| {
            assert_eq!(
                ioreq.address_space,
                AddressSpace::PciConfig {
                    device: device as u8
                }
            );
            assert_eq!(ioreq.address, address);
            assert_eq!(ioreq.size, size);
            assert_eq!(
                ioreq.operation,
                IoOperation::Write {
                    data: data.to_ne_bytes()
                }
            );

            None
        };
        assert!(rpc.process_ioreq(&mut process_fn).is_ok());

        let rpcmsg = dequeue_ioreq_reply(&rpc);
        assert_rpcmsg_eq(&rpcmsg, direction, device, slot, address, size, data);

        assert!(rpc.process_ioreq(&mut expect_no_ioreqs()).is_err());
    }

    #[test]
    fn test_process_pci_config_read() {
        let rpc = create_rpc();
        let direction = RPC_MR0_MMIO_DIRECTION_READ;
        let device = 7;
        let slot = 3;
        let address = 0xabba_cafe;
        let size = 4;
        let data = 0x1234_5678;
        let read_data = 0x1111_2222u64;

        enqueue_ioreq(&rpc, direction, device, slot, address, size, data);

        let mut process_fn = |ioreq: IoRequest| {
            assert_eq!(
                ioreq.address_space,
                AddressSpace::PciConfig {
                    device: device as u8
                }
            );
            assert_eq!(ioreq.address, address);
            assert_eq!(ioreq.size, size);
            assert_eq!(ioreq.operation, IoOperation::Read);

            Some(read_data.to_ne_bytes())
        };
        assert!(rpc.process_ioreq(&mut process_fn).is_ok());

        let rpcmsg = dequeue_ioreq_reply(&rpc);
        assert_rpcmsg_eq(&rpcmsg, direction, device, slot, address, size, read_data);

        assert!(rpc.process_ioreq(&mut expect_no_ioreqs()).is_err());
    }
}
