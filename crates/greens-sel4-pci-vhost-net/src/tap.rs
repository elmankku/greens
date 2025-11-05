// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::fs::{File, OpenOptions};
use std::io::Error as IoError;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, RawFd};

use libc::{IFF_NO_PI, IFF_TAP, IFF_VNET_HDR};
use vmm_sys_util::ioctl::{ioctl_with_mut_ref, ioctl_with_ref, ioctl_with_val};

use crate::error::{Error, Result};

const TUN_DEVICE: &str = "/dev/net/tun";
const IFACE_MAX_LEN: usize = libc::IF_NAMESIZE;

/// TAP virtual network interface.
///
/// Wraps the open file descriptor and provides methods for configuring the underlying TAP interface.
#[derive(Debug)]
pub(crate) struct Tap {
    /// The file handle for underlying TAP device
    pub(crate) file: File,
}

impl Tap {
    /// Open a new TAP device by `if_name`.
    pub fn open_named(if_name: &str) -> Result<Tap> {
        let tap = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(TUN_DEVICE)
            .map_err(|e| {
                Error::TapConfiguration(format!("failed to open {}: {}", TUN_DEVICE, e))
            })?;

        let mut ifreq = ifreq_with_flags(if_name, IFF_TAP | IFF_NO_PI | IFF_VNET_HDR)?;

        // SAFETY: called with a valid file and the result is checked.
        let ret = unsafe { ioctl_with_mut_ref(&tap, libc::TUNSETIFF, &mut ifreq) };
        if ret < 0 {
            let msg = format!("tunsetiff ioctl failed: {}", IoError::last_os_error());
            return Err(Error::TapConfiguration(msg));
        }

        Ok(Tap { file: tap })
    }

    /// Set the offload flags for the tap device.
    pub fn set_offload(&self, flags: u32) -> Result<()> {
        let flags = libc::c_ulong::from(flags);

        // SAFETY: called with a valid file and the result is checked.
        let ret = unsafe { ioctl_with_val(&self.file, libc::TUNSETOFFLOAD, flags) };
        if ret < 0 {
            return Err(Error::TapConfiguration(format!(
                "set tap offload flags: {}",
                IoError::last_os_error()
            )));
        }

        Ok(())
    }

    /// Set the vnet haeder size for the tap device.
    pub fn set_vnet_hdr_size(&self, size: u32) -> Result<()> {
        let size = libc::c_int::from(size as i32);

        // SAFETY: called with a valid file and the result is checked.
        let ret = unsafe { ioctl_with_ref(&self.file, libc::TUNSETVNETHDRSZ, &size) };
        if ret < 0 {
            return Err(Error::TapConfiguration(format!(
                "set vnet header size: {}",
                IoError::last_os_error()
            )));
        }

        Ok(())
    }
}

impl AsRawFd for Tap {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

fn null_term_if_name(name: &str) -> Result<[libc::c_char; IFACE_MAX_LEN]> {
    let name = name.as_bytes();

    if name.len() > IFACE_MAX_LEN {
        return Err(Error::TapNameTooLong {
            len: name.len(),
            max: IFACE_MAX_LEN,
        });
    }
    let mut terminated = [b'\0'; IFACE_MAX_LEN];
    terminated[..name.len()].copy_from_slice(name);

    Ok(std::array::from_fn(|i| terminated[i] as libc::c_char))
}

fn ifreq_with_flags(if_name: &str, flags: i32) -> Result<libc::ifreq> {
    assert!(flags <= i16::MAX as i32, "invalid ifreq flags");

    let ifreq = libc::ifreq {
        ifr_name: null_term_if_name(if_name)?,
        ifr_ifru: libc::__c_anonymous_ifr_ifru {
            ifru_flags: flags as i16,
        },
    };

    Ok(ifreq)
}
