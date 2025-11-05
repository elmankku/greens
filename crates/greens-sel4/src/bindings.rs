// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(unused_imports)]

use vmm_sys_util::{ioctl_io_nr, ioctl_ioc_nr, ioctl_iow_nr};

// include generated FFI bindings
include!(concat!(env!("OUT_DIR"), "/sel4_virt_generated.rs"));

unsafe impl Send for vso_rpc {}
unsafe impl Sync for vso_rpc {}

// ioctls
ioctl_iow_nr!(SEL4_CREATE_VM, SEL4_IOCTL, 0x20, sel4_vm_params);
ioctl_io_nr!(SEL4_START_VM, SEL4_IOCTL, 0x21);
ioctl_iow_nr!(SEL4_CREATE_VPCI_DEVICE, SEL4_IOCTL, 0x22, sel4_vpci_device);
ioctl_iow_nr!(SEL4_DESTROY_VPCI_DEVICE, SEL4_IOCTL, 0x23, sel4_vpci_device);
ioctl_iow_nr!(SEL4_SET_IRQLINE, SEL4_IOCTL, 0x24, sel4_irqline);

ioctl_iow_nr!(SEL4_IOEVENTFD, SEL4_IOCTL, 0x25, sel4_ioeventfd_config);
ioctl_iow_nr!(SEL4_IRQFD, SEL4_IOCTL, 0x26, sel4_irqfd_config);

ioctl_iow_nr!(SEL4_CREATE_IO_HANDLER, SEL4_IOCTL, 0x30, __u64);
ioctl_io_nr!(SEL4_WAIT_IO, SEL4_IOCTL, 0x31);

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::Layout;
    use std::alloc::{alloc_zeroed, dealloc};
    use std::os::raw::c_void;

    const COOKIE_VAL: u32 = 42;

    extern "C" fn doorbell(cookie_ptr: *mut c_void) {
        let cookie: &u32 = unsafe { &*(cookie_ptr as *const u32) };

        assert_eq!(cookie.to_owned(), COOKIE_VAL);
    }

    #[test]
    fn test_bindings_rpc_send() {
        let mut cookie: u32 = COOKIE_VAL;
        let cookie_ptr: *mut c_void = &mut cookie as *mut _ as *mut c_void;
        let mut rpc = vso_rpc::default();
        let rpc_ptr = &mut rpc as *mut _;

        let size = 4096 * IOBUF_NUM_PAGES as usize;
        let layout = Layout::from_size_align(size, 4096).expect("invalid layout");

        unsafe {
            let buf = alloc_zeroed(layout);

            let result = vso_rpc_init(
                rpc_ptr,
                vso_rpc_id_vso_rpc_device,
                buf as *mut _,
                Some(doorbell),
                cookie_ptr,
            );
            assert_eq!(result, 0);

            let result = device_rpc_req_start_vm(&mut rpc as *mut _);
            assert_eq!(result, 0);

            dealloc(buf, layout);
        };
    }
}
