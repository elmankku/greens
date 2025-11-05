// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

#[cfg(all(not(feature = "std"), not(test)))]
extern crate core as std;

pub mod io_interface;
pub mod ioreq;
