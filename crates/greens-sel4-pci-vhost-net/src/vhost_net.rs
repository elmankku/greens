// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::borrow::Borrow;
use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use greens_pci::PciMsiMessage;
use greens_pci_virtio::pci::VirtioPciDevice;
use greens_pci_virtio::pci_common_cfg::MSIX_VECTOR_UNMAPPED;
use greens_pci_virtio::pci_isr_cfg::VirtioPciIsrState;
use greens_pci_virtio::pci_notify_cfg::VirtioPciNotifyCfgInfo;
use greens_sys_linux::eventfd::EventFdBinder;
use greens_sys_linux::eventfd::IoEventFdConfig;
use greens_sys_linux::eventfd::IrqFdConfig;
use vhost::VhostBackend;
use vhost::VhostUserMemoryRegionInfo;
use vhost::VringConfigData;
use vhost::net::VhostNet;
use vhost::vhost_kern::VhostKernBackend;
use vhost::vhost_kern::net::Net;
use virtio_bindings::virtio_config::VIRTIO_F_ACCESS_PLATFORM;
use virtio_bindings::virtio_config::VIRTIO_F_NOTIFICATION_DATA;
use virtio_bindings::virtio_config::VIRTIO_F_RING_RESET;
use virtio_bindings::virtio_ids;
use virtio_bindings::virtio_net::VIRTIO_NET_F_CSUM;
use virtio_bindings::virtio_net::VIRTIO_NET_F_GUEST_CSUM;
use virtio_bindings::virtio_net::VIRTIO_NET_F_GUEST_TSO4;
use virtio_bindings::virtio_net::VIRTIO_NET_F_GUEST_TSO6;
use virtio_bindings::virtio_net::VIRTIO_NET_F_GUEST_UFO;
use virtio_bindings::virtio_net::VIRTIO_NET_F_HOST_TSO4;
use virtio_bindings::virtio_net::VIRTIO_NET_F_HOST_TSO6;
use virtio_bindings::virtio_net::VIRTIO_NET_F_HOST_UFO;
use virtio_bindings::virtio_net::VIRTIO_NET_F_MAC;
use virtio_device::VirtioDevice;
use virtio_device::WithDriverSelect;
use virtio_device::{VirtioConfig, VirtioDeviceActions, VirtioDeviceType};
use virtio_queue::{Queue, QueueT};
use vm_memory::Address;
use vm_memory::GuestAddressSpace;
use vm_memory::GuestMemory;
use vm_memory::GuestMemoryMmap;
use vm_memory::GuestMemoryRegion;
use vmm_sys_util::eventfd::EFD_NONBLOCK;
use vmm_sys_util::eventfd::EventFd;

use crate::tap::Tap;

// This is from kernel
const VHOST_F_LOG_ALL: u32 = 26;

// VirtIO packet header in bytes.
const VIRTIO_NET_HDR_LEN: u32 = 12;

pub(crate) struct VhostNetDevice<T: EventFdBinder> {
    pub net: Net<Arc<GuestMemoryMmap>>,
    pub cfg: VirtioConfig<Queue>,
    pub config_msix_vector: u16,
    pub queue_msix_vector: Vec<u16>,
    pub notify_cfg_info: Option<VirtioPciNotifyCfgInfo>,
    pub msix_data: HashMap<u16, u32>,
    pub binder: Arc<T>,
    pub ioeventfds: Vec<EventFd>,
    pub irqfds: Vec<EventFd>,
    pub tap: Tap,
}

pub(crate) struct VhostNetConfig {
    pub tap: String,
    pub mac: Option<[u8; 6]>,
    pub queue_size: u16,
}

impl<T> VirtioPciDevice for VhostNetDevice<T>
where
    T: EventFdBinder,
{
    fn set_config_msix_vector(&mut self, vector: u16) {
        self.config_msix_vector = vector;
    }

    fn config_msix_vector(&self) -> u16 {
        self.config_msix_vector
    }

    fn set_notification_info(&mut self, notify_cfg_info: VirtioPciNotifyCfgInfo) {
        self.notify_cfg_info = Some(notify_cfg_info)
    }

    fn queue_notify(&mut self, data: u32) {
        if let Some(fd) = self.ioeventfds.get_mut(data as usize) {
            let eventfd: &EventFd = fd;
            // FIXME: error... what to do?
            eventfd.write(1).unwrap();
        }
    }

    fn set_msi_message(&mut self, vector: u16, msg: PciMsiMessage) {
        self.msix_data.insert(vector, msg.data);
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
}

impl<T> VirtioPciIsrState for VhostNetDevice<T>
where
    T: EventFdBinder,
{
    fn read_and_clear_isr(&mut self) -> u8 {
        self.interrupt_status().swap(0, Ordering::AcqRel)
    }
}

fn feature_mask(bits: Vec<u32>) -> u64 {
    bits.iter().fold(0, |mask, bit| mask | (1 << bit))
}

// Features advertised for the guest, but not with backend.
fn transport_features() -> u64 {
    // VIRTIO_F_ACCESS_PLATFORM is advertised for the guest but not for the backend, because
    // that enables iotlb. That will fail unless properly handled.
    feature_mask(vec![VIRTIO_F_ACCESS_PLATFORM])
}

fn supported_device_features() -> u64 {
    let features = vec![
        VIRTIO_NET_F_CSUM,
        VIRTIO_NET_F_GUEST_CSUM,
        VIRTIO_NET_F_GUEST_TSO4,
        VIRTIO_NET_F_GUEST_TSO6,
        VIRTIO_NET_F_GUEST_UFO,
        VIRTIO_NET_F_HOST_TSO4,
        VIRTIO_NET_F_HOST_TSO6,
        VIRTIO_NET_F_HOST_UFO,
        VIRTIO_NET_F_MAC,
    ];

    feature_mask(features)
}

fn device_features(config: &VhostNetConfig) -> u64 {
    let mut features = supported_device_features();

    if config.mac.is_none() {
        features &= !(1 << VIRTIO_NET_F_MAC);
    }

    features
}

impl<T> VhostNetDevice<T>
where
    T: EventFdBinder,
{
    pub(crate) fn new(
        binder: Arc<T>,
        guest_memory: Arc<GuestMemoryMmap>,
        config: VhostNetConfig,
    ) -> Self {
        let net = Net::new(guest_memory.clone()).unwrap();

        net.set_owner().expect("set owner");

        let disabled_features = !feature_mask(vec![VHOST_F_LOG_ALL, VIRTIO_F_RING_RESET]);
        let backend_features = net.get_features().unwrap();
        let device_features = device_features(&config);

        // Features advertised to the driver: we disable the features that are not supported.
        let features =
            disabled_features & (backend_features | transport_features() | device_features);

        let queues = vec![
            Queue::new(config.queue_size).unwrap(),
            Queue::new(config.queue_size).unwrap(),
        ];

        let tap = Tap::open_named(&config.tap).expect("tap open");
        tap.set_vnet_hdr_size(VIRTIO_NET_HDR_LEN)
            .expect("virtio header size");

        let mac = config.mac.unwrap_or([0u8; 6]).to_vec();

        let cfg = VirtioConfig::new(features, queues, mac);

        Self {
            cfg,
            config_msix_vector: 0,
            queue_msix_vector: vec![MSIX_VECTOR_UNMAPPED, MSIX_VECTOR_UNMAPPED],
            notify_cfg_info: None,
            msix_data: HashMap::new(),
            net,
            binder,
            // FIXME: deassign on Drop
            ioeventfds: vec![],
            irqfds: vec![],
            tap,
        }
    }

    fn feature_negotiated(&self, feature: u32) -> bool {
        self.driver_features() & (1 << feature) != 0
    }
}

impl<T> VirtioDeviceType for VhostNetDevice<T>
where
    T: EventFdBinder,
{
    fn device_type(&self) -> u32 {
        virtio_ids::VIRTIO_ID_NET
    }
}

impl<T> Borrow<VirtioConfig<Queue>> for VhostNetDevice<T>
where
    T: EventFdBinder,
{
    fn borrow(&self) -> &VirtioConfig<Queue> {
        &self.cfg
    }
}

impl<T> BorrowMut<VirtioConfig<Queue>> for VhostNetDevice<T>
where
    T: EventFdBinder,
{
    fn borrow_mut(&mut self) -> &mut VirtioConfig<Queue> {
        &mut self.cfg
    }
}

impl<T> VirtioDeviceActions for VhostNetDevice<T>
where
    T: EventFdBinder,
{
    // FIXME: we need proper errors...
    type E = greens_pci::Error;

    fn activate(&mut self) -> Result<(), Self::E> {
        // FIXME: propagate errors.
        // FIXME: Figure out how we should store the features.

        // Set backend features; take negotiated features, remove transport features and device
        // specific features that it should not be aware of.
        let mut backend_features = self.driver_features();
        backend_features &= !transport_features();
        backend_features &= !supported_device_features();
        self.net
            .set_features(backend_features)
            .expect("set backend features");

        // Set negotiated features to TAP device
        let mut tap_offload = 0;
        if self.feature_negotiated(VIRTIO_NET_F_CSUM) {
            tap_offload |= libc::TUN_F_CSUM;
        }
        if self.feature_negotiated(VIRTIO_NET_F_HOST_UFO) {
            tap_offload |= libc::TUN_F_UFO;
        }
        if self.feature_negotiated(VIRTIO_NET_F_HOST_TSO4) {
            tap_offload |= libc::TUN_F_TSO4;
        }
        if self.feature_negotiated(VIRTIO_NET_F_HOST_TSO6) {
            tap_offload |= libc::TUN_F_TSO6;
        }
        self.tap.set_offload(tap_offload).expect("set TAP offload");

        // The ioeventfd access size is 2 bytes if VIRTIO_F_NOTIFICATION_DATA has not
        // been negotiated.
        let ioeventfd_size = if self.feature_negotiated(VIRTIO_F_NOTIFICATION_DATA) {
            4
        } else {
            2
        };

        let info = memory_region_info(self.net.mem());
        self.net.set_mem_table(&info).expect("mem table");

        self.cfg
            .queues
            .iter()
            .zip(self.queue_msix_vector.iter())
            .enumerate()
            .for_each(|(i, (q, vector))| {
                // FIXME: this must be checked - otherwise error.
                // if *vector != MSIX_VECTOR_UNMAPPED {

                // Set irqfd
                let efd = EventFd::new(EFD_NONBLOCK).expect("create eventfd");
                let irqfd = IrqFdConfig::new(
                    efd.as_raw_fd(),
                    *self.msix_data.get(vector).expect("msix data"),
                );
                self.binder.bind_irqfd(&irqfd).expect("assign irqfd");
                self.net.set_vring_call(i, &efd).expect("set vring irqfd");
                self.irqfds.push(efd);

                // FIXME: Split parts to functions?
                self.net.set_vring_num(i, q.size()).unwrap();

                let memory: &GuestMemoryMmap = &self.net.mem().memory();
                let base = q.avail_idx(memory, Ordering::Acquire).unwrap().0;
                self.net.set_vring_base(i, base).unwrap();

                let config = VringConfigData {
                    queue_max_size: q.max_size(),
                    queue_size: q.size(),
                    flags: 0,
                    desc_table_addr: q.desc_table(),
                    used_ring_addr: q.used_ring(),
                    avail_ring_addr: q.avail_ring(),
                    log_addr: None,
                };
                self.net.set_vring_addr(i, &config).expect("set vring addr");

                // Configure ioeventfd
                let efd = EventFd::new(EFD_NONBLOCK).unwrap();
                let gpa = self
                    .notify_cfg_info
                    .as_ref()
                    .expect("notification info not set")
                    .notification_gpa(i as u16)
                    .expect("notification gpa out of range");
                let ioeventfd = IoEventFdConfig::new(efd.as_raw_fd(), gpa, ioeventfd_size, None);

                self.binder
                    .bind_ioeventfd(&ioeventfd)
                    .expect("assign ioeventfd");
                self.net
                    .set_vring_kick(i, &efd)
                    .expect("set vring ioeventfd");
                self.ioeventfds.push(efd);

                self.net
                    .set_backend(i, Some(&self.tap.file))
                    .expect("set tap backend");
            });

        Ok(())
    }

    fn reset(&mut self) -> Result<(), Self::E> {
        // FIXME
        Ok(())
    }
}

// FIXME: Result + map_err()
fn memory_region_info(guest_memory: &GuestMemoryMmap) -> Vec<VhostUserMemoryRegionInfo> {
    guest_memory
        .iter()
        .map(|r| {
            let file_offset = r.file_offset().unwrap();
            VhostUserMemoryRegionInfo {
                guest_phys_addr: r.start_addr().raw_value(),
                memory_size: r.size() as u64,
                userspace_addr: r.as_ptr() as u64,
                mmap_offset: file_offset.start(),
                mmap_handle: file_offset.file().as_raw_fd(),
            }
        })
        .collect()
}
