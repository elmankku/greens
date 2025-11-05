// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::ptr::null_mut;

use libc::c_int;

pub type Result<T> = std::result::Result<T, io::Error>;

#[derive(Debug)]
pub struct MemoryMapping {
    pub addr: *mut u8,
    pub size: usize,
}

unsafe impl Send for MemoryMapping {}
unsafe impl Sync for MemoryMapping {}

/// # Safety
///
/// Safe when implementers guarantee that `ptr`..`ptr+size` is an mmapped region owned by this
/// object and can't be unmapped during `MemoryRegion`'s lifetime.
pub unsafe trait MemoryRegion: Send + Sync {
    fn as_ptr(&self) -> *mut u8;
    fn size(&self) -> usize;
}

unsafe impl MemoryRegion for MemoryMapping {
    fn as_ptr(&self) -> *mut u8 {
        self.addr
    }

    fn size(&self) -> usize {
        self.size
    }
}

impl MemoryMapping {
    pub fn try_mmap(
        addr: Option<*mut u8>,
        size: usize,
        prot: c_int,
        flags: c_int,
        file: Option<&File>,
        offset: Option<i64>,
    ) -> Result<MemoryMapping> {
        let addr = match addr {
            Some(addr) => addr as *mut libc::c_void,
            None => null_mut(),
        };
        let fd = match file {
            Some(f) => f.as_raw_fd(),
            None => -1,
        };

        let addr = unsafe { libc::mmap64(addr, size, prot, flags, fd, offset.unwrap_or(0)) };
        if addr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        Ok(MemoryMapping {
            addr: addr as *mut u8,
            size,
        })
    }
}

impl Drop for MemoryMapping {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.addr as *mut libc::c_void, self.size);
        }
    }
}
