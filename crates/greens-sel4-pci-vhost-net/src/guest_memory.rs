// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::fs::File;
use std::io;
use std::os::fd::AsFd;

use greens_sel4::io_interface::GuestMemoryRegion;
use vm_memory::mmap::MmapRegionError;
use vm_memory::{
    FileOffset, GuestAddress, GuestMemoryMmap, GuestRegionCollectionError, GuestRegionMmap,
    MmapRegion,
};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to clone guest RAM file descriptor: {0}")]
    FdClone(io::Error),
    #[error("failed to mmap guest RAM: {0}")]
    Mmap(MmapRegionError),
    #[error("guest RAM address overflow at {0:#x}")]
    AddressOverflow(u64),
    #[error("failed to build guest memory: {0}")]
    Build(GuestRegionCollectionError),
}

pub fn build_guest_memory(regions: &[GuestMemoryRegion]) -> Result<GuestMemoryMmap, Error> {
    let regions = regions
        .iter()
        .map(|region| {
            let file = File::from(
                region
                    .fd
                    .as_fd()
                    .try_clone_to_owned()
                    .map_err(Error::FdClone)?,
            );

            let mmap = MmapRegion::build(
                Some(FileOffset::new(file, region.fd_offset)),
                region.size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
            )
            .map_err(Error::Mmap)?;

            GuestRegionMmap::new(mmap, GuestAddress(region.guest_addr))
                .ok_or(Error::AddressOverflow(region.guest_addr))
        })
        .collect::<Result<Vec<_>, _>>()?;

    GuestMemoryMmap::from_regions(regions).map_err(Error::Build)
}
