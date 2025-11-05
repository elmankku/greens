// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::ops::Deref;
use std::os::raw::c_void;

use greens_sys_linux::mmap::MemoryRegion;

use super::RpcError;

pub trait Doorbell {
    fn cookie(&self) -> *mut c_void;
    extern "C" fn ring(cookie: *mut c_void);
}

#[derive(Debug, PartialEq)]
pub struct MmioDoorbell<T> {
    region: T,
    offset: usize,
}

impl<T> MmioDoorbell<T>
where
    T: MemoryRegion,
{
    pub fn new(region: T, offset: Option<usize>) -> Result<Self, RpcError> {
        let offset = offset.unwrap_or(0);
        match offset < region.size() {
            true => Ok(Self { region, offset }),
            false => {
                let e = MemoryRegionOffsetError {
                    offset,
                    size: region.size(),
                };
                Err(e.into())
            }
        }
    }
}

impl<T> Deref for MmioDoorbell<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.region
    }
}

impl<T> Doorbell for MmioDoorbell<T>
where
    T: MemoryRegion,
{
    extern "C" fn ring(addr: *mut c_void) {
        // SAFETY: Volatile write is safe as offset of `addr` is checked.
        unsafe { std::ptr::write_volatile(addr as *mut u32, 1) };
    }

    fn cookie(&self) -> *mut c_void {
        // SAFETY: pointer arithmetic is safe because offset is checked against memory region
        // size at initialization.
        unsafe { self.as_ptr().add(self.offset) as _ }
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("requested offset `{offset}` exceeds region size `{size}`")]
pub struct MemoryRegionOffsetError {
    pub offset: usize,
    pub size: usize,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::rpc::tests::TestMapping;
    use std::alloc::Layout;

    #[test]
    fn test_doorbell() {
        let layout: Layout = Layout::new::<u32>();
        let mapping = TestMapping::new(layout);
        let doorbell = MmioDoorbell::new(mapping, None).unwrap();

        assert_eq!(unsafe { *(doorbell.as_ptr()) }, 0);

        MmioDoorbell::<TestMapping>::ring(doorbell.cookie());
        assert_eq!(unsafe { *(doorbell.as_ptr()) }, 1);
    }

    #[test]
    fn test_doorbell_offset() {
        let offset = 4;
        let size = std::mem::size_of::<u32>();

        let layout: Layout = Layout::from_size_align(offset + size, size).expect("invalid layout");
        let mapping = TestMapping::new(layout);
        let _ = MmioDoorbell::new(mapping, Some(offset)).unwrap();
    }

    #[test]
    #[should_panic(expected = "MemoryRegionOffsetError { offset: 8, size: 4 }")]
    fn test_doorbell_invalid_offset() {
        let offset = 8;
        let layout: Layout = Layout::new::<u32>();
        let mapping = TestMapping::new(layout);
        let _ = MmioDoorbell::new(mapping, Some(offset)).unwrap();
    }
}
