// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use greens_core::io_interface::{InterruptLine, InterruptLineOperation, IoInterface, MsiMessage};
use greens_core::ioreq::IoRequest;
use greens_sys_linux::eventfd::{EventFdBinder, IoEventFdConfig, IrqFdConfig};
use greens_sys_linux::mmap::MemoryMapping;
use vm_memory::guest_memory::FileOffset;
use vm_memory::{GuestAddress, GuestMemoryMmap, GuestRegionMmap, MmapRegion};
use vmm_sys_util::ioctl::{ioctl, ioctl_with_ptr, ioctl_with_val};

use crate::bindings::{sel4_ioeventfd_config, sel4_irqfd_config};
use crate::bindings::{sel4_irqline, sel4_vm_params, sel4_vpci_device};
use crate::bindings::{
    IOBUF_NUM_PAGES, SEL4_IRQ_OP_CLR, SEL4_IRQ_OP_PULSE, SEL4_IRQ_OP_SET, SEL4_MEM_MAP_EVENT_BAR,
    SEL4_MEM_MAP_IOBUF, SEL4_MEM_MAP_RAM,
};
use crate::bindings::{RPC_ADDRESS_SPACE_GLOBAL, SEL4_IRQFD};
use crate::bindings::{
    SEL4_CREATE_IO_HANDLER, SEL4_CREATE_VM, SEL4_CREATE_VPCI_DEVICE, SEL4_SET_IRQLINE,
    SEL4_START_VM, SEL4_WAIT_IO,
};
use crate::bindings::{SEL4_IOEVENTFD, SEL4_IRQFD_FLAG_DEASSIGN};
use crate::bindings::{SEL4_IOEVENTFD_FLAG_DATAMATCH, SEL4_IOEVENTFD_FLAG_DEASSIGN};
use crate::io_interface_config::io_interface_config;
use crate::rpc::{DriverRpc, MmioDoorbell, RpcError};

type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub enum Error {
    #[error("failed to open device: {0}")]
    DeviceOpen(io::Error),
    #[error("failed to attach vm: {0}")]
    AttachVm(io::Error),
    #[error("creating io handler for {kind} failed: {source}")]
    CreateIoHandler {
        kind: IoMapKindInfo,
        #[source]
        source: io::Error,
    },
    #[error("failed to mmap io handler {kind}: {source}")]
    MmapIoHandler {
        kind: IoMapKindInfo,
        #[source]
        source: io::Error,
    },
    #[error("failed to mmap guest ram: {0}")]
    MmapGuestRam(#[from] vm_memory::mmap::MmapRegionError),
    GuestRam(#[from] vm_memory::Error),
    #[error("failed to create vpci device: {0}")]
    CreateVpciDevice(io::Error),
    #[error("failed to signal ready: {0}")]
    SignalReady(io::Error),
    #[error("failed to wait io: {0}")]
    WaitIo(io::Error),
    #[error("failed to communicate over rpc: {0}")]
    Rpc(#[from] RpcError),
    #[error("failed to set interrupt line: {0}")]
    SetInterrupt(io::Error),
    #[error("failed to create ioeventfd: {0}")]
    IoEventFd(io::Error),
    #[error("failed to create irqfd: {0}")]
    IrqFd(io::Error),
    #[error("failed to acquire lock")]
    LockError,
    #[error("failed to get io connection config: {0}")]
    IoConnectionConfig(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u64)]
enum IoMapKind {
    Ram = SEL4_MEM_MAP_RAM as u64,
    IoBuf = SEL4_MEM_MAP_IOBUF as u64,
    Event = SEL4_MEM_MAP_EVENT_BAR as u64,
}

impl IoMapKind {
    fn as_str(&self) -> &'static str {
        match self {
            IoMapKind::Ram => "ram",
            IoMapKind::IoBuf => "io",
            IoMapKind::Event => "notification",
        }
    }
}

#[derive(Debug)]
pub struct IoMapKindInfo {
    kind: &'static str,
}

impl From<IoMapKind> for IoMapKindInfo {
    fn from(kind: IoMapKind) -> Self {
        Self {
            kind: kind.as_str(),
        }
    }
}

impl fmt::Display for IoMapKindInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)
    }
}

pub type PciDeviceNumber = u32;

#[derive(Debug)]
pub struct Sel4IoInterface {
    #[allow(dead_code)]
    device: File,
    control: File,
    guest_memory: Arc<GuestMemoryMmap<()>>,
    rpc: DriverRpc<MemoryMapping, MmioDoorbell<MemoryMapping>>,
    num_pcidevs: Arc<Mutex<PciDeviceNumber>>,
    ready: Arc<Mutex<bool>>,
}

impl Sel4IoInterface {
    pub fn new(device_path: PathBuf, device_model_id: u64) -> Result<Self> {
        let io_cfg = io_interface_config(device_model_id)
            .map_err(|e| Error::IoConnectionConfig(e.to_string()))?;
        let vm_params = sel4_vm_params {
            ram_size: io_cfg.ram_size,
            id: device_model_id,
        };

        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(device_path)
            .map_err(Error::DeviceOpen)?;

        let fd = unsafe { ioctl_with_ptr(&device, SEL4_CREATE_VM(), &vm_params as *const _) };
        if fd == -1 {
            return Err(Error::AttachVm(io::Error::last_os_error()));
        }
        let control = unsafe { File::from_raw_fd(fd) };

        // Map guest RAM pages
        let regions = Self::map_guest_ram(
            &control,
            None,
            vm_params.ram_size as usize,
            io_cfg.ram_start,
        )?;
        let guest_memory = Arc::new(GuestMemoryMmap::from_regions(regions)?);

        // Map RPC pages
        let io_mapping =
            Self::map_buffer(&control, IoMapKind::IoBuf, IOBUF_NUM_PAGES as usize * 4096)?;
        let event_mapping = Self::map_buffer(&control, IoMapKind::Event, 4096)?;
        let doorbell = MmioDoorbell::new(event_mapping, None)?;

        let rpc = DriverRpc::new(io_mapping, doorbell)?;

        Ok(Self {
            device,
            control,
            guest_memory,
            rpc,
            num_pcidevs: Arc::new(Mutex::new(0)),
            ready: Arc::new(Mutex::new(false)),
        })
    }

    pub fn guest_memory(&self) -> &Arc<GuestMemoryMmap<()>> {
        &self.guest_memory
    }

    pub fn signal_ready(&self) -> Result<()> {
        let mut ready = self.ready.lock().map_err(|_| Error::LockError)?;
        if *ready {
            return Ok(());
        }

        match unsafe { ioctl(&self.control, SEL4_START_VM()) } {
            -1 => Err(Error::SignalReady(io::Error::last_os_error())),
            _ => {
                *ready = true;

                Ok(())
            }
        }
    }

    pub fn register_vpci_device(&self) -> Result<PciDeviceNumber> {
        let mut pcidev = self.num_pcidevs.lock().map_err(|_| Error::LockError)?;
        let dev = sel4_vpci_device { pcidev: *pcidev };

        match unsafe { ioctl_with_ptr(&self.control, SEL4_CREATE_VPCI_DEVICE(), &dev as *const _) }
        {
            -1 => Err(Error::CreateVpciDevice(io::Error::last_os_error())),
            _ => {
                *pcidev += 1;
                Ok(dev.pcidev)
            }
        }
    }

    fn map_guest_ram(
        control: &File,
        start: Option<u64>,
        size: usize,
        guest_addr: u64,
    ) -> Result<Vec<GuestRegionMmap>> {
        let fd =
            unsafe { ioctl_with_val(control, SEL4_CREATE_IO_HANDLER(), IoMapKind::Ram as u64) };
        if fd < 0 {
            return Err(Error::CreateIoHandler {
                kind: IoMapKind::Ram.into(),
                source: io::Error::last_os_error(),
            });
        }
        let file = unsafe { File::from_raw_fd(fd) };

        let region = MmapRegion::build(
            Some(FileOffset::new(file, start.unwrap_or(0))),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
        )?;

        let region = GuestRegionMmap::new(region, GuestAddress(guest_addr))?;

        Ok(vec![region])
    }

    fn map_buffer(control: &File, kind: IoMapKind, size: usize) -> Result<MemoryMapping> {
        let fd = unsafe { ioctl_with_val(control, SEL4_CREATE_IO_HANDLER(), kind as u64) };
        if fd < 0 {
            return Err(Error::CreateIoHandler {
                kind: kind.into(),
                source: io::Error::last_os_error(),
            });
        }

        let file = unsafe { File::from_raw_fd(fd) };
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        let flags = libc::MAP_SHARED;

        MemoryMapping::try_mmap(None, size, prot, flags, Some(&file), None).map_err(|e| {
            Error::MmapIoHandler {
                kind: kind.into(),
                source: e,
            }
        })
    }

    fn wait_io(&self) -> Result<()> {
        // FIXME: First check if there are messages in the queue
        // If not, do ioctl. When ioctl returns, return message.
        // Do this in a loop.
        match unsafe { ioctl(&self.control, SEL4_WAIT_IO()) } {
            // FIXME: Error kind
            -1 => Err(Error::WaitIo(io::Error::last_os_error())),
            _ => Ok(()),
        }
    }
}

impl IoInterface for Sel4IoInterface {
    type E = Error;

    fn handle_next_io(
        &self,
        io_handle_fn: &mut dyn FnMut(IoRequest) -> Option<[u8; 8]>,
    ) -> Result<()> {
        self.signal_ready()?;

        self.wait_io()?;
        self.rpc.process_ioreq(io_handle_fn)?;

        Ok(())
    }

    fn set_interrupt(
        &self,
        irq: InterruptLine,
        op: InterruptLineOperation,
    ) -> std::result::Result<(), Self::E> {
        // FIXME: use direct rpc message
        let op = match op {
            InterruptLineOperation::Clear => SEL4_IRQ_OP_CLR,
            InterruptLineOperation::Set => SEL4_IRQ_OP_SET,
            InterruptLineOperation::Pulse => SEL4_IRQ_OP_PULSE,
        };

        let irqline = sel4_irqline { irq, op };
        match unsafe { ioctl_with_ptr(&self.control, SEL4_SET_IRQLINE(), &irqline as *const _) } {
            -1 => Err(Error::SetInterrupt(io::Error::last_os_error())),
            _ => Ok(()),
        }
    }

    fn send_msi(&self, message: MsiMessage) -> std::result::Result<(), Self::E> {
        let gsi = message.data & 0xFFFF;

        self.set_interrupt(gsi, InterruptLineOperation::Pulse)
    }
}

impl EventFdBinder for Sel4IoInterface {
    type E = Error;

    fn bind_ioeventfd(&self, config: &IoEventFdConfig) -> std::result::Result<(), Self::E> {
        ioeventfd_config(&self.control, config, true)
    }

    fn unbind_ioeventfd(&self, config: &IoEventFdConfig) -> std::result::Result<(), Self::E> {
        ioeventfd_config(&self.control, config, false)
    }

    fn bind_irqfd(&self, config: &IrqFdConfig) -> std::result::Result<(), Self::E> {
        irqfd_config(&self.control, config, true)
    }

    fn unbind_irqfd(&self, config: &IrqFdConfig) -> std::result::Result<(), Self::E> {
        irqfd_config(&self.control, config, false)
    }
}

fn ioeventfd_config<F: AsRawFd>(f: &F, config: &IoEventFdConfig, bind: bool) -> Result<()> {
    let mut flags = if config.data.is_some() {
        SEL4_IOEVENTFD_FLAG_DATAMATCH
    } else {
        0
    };

    if !bind {
        flags |= SEL4_IOEVENTFD_FLAG_DEASSIGN;
    }

    let params = sel4_ioeventfd_config {
        fd: config.fd,
        flags,
        addr: config.addr,
        len: config.len,
        addr_space: RPC_ADDRESS_SPACE_GLOBAL,
        data: config.data.unwrap_or_default(),
    };

    match unsafe { ioctl_with_ptr(f, SEL4_IOEVENTFD(), &params as *const _) } {
        -1 => Err(Error::IoEventFd(io::Error::last_os_error())),
        _ => Ok(()),
    }
}

fn irqfd_config<F: AsRawFd>(f: &F, config: &IrqFdConfig, bind: bool) -> Result<()> {
    let flags = if !bind { SEL4_IRQFD_FLAG_DEASSIGN } else { 0 };

    let params = sel4_irqfd_config {
        fd: config.fd,
        flags,
        virq: config.irq,
    };

    match unsafe { ioctl_with_ptr(f, SEL4_IRQFD(), &params as *const _) } {
        -1 => Err(Error::IrqFd(io::Error::last_os_error())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {}
