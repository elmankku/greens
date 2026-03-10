// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::Result;
use crate::configuration_space::PciConfigurationSpace;
use crate::function::PciHandlerResult;

/// A trait to handle read and write operations to PCI configuration space.
///
/// The trait is designed for PCI configuration space I/O pre/post processing,
/// where context typically would contain `&'a mut PciConfigurationSpace`
/// bundled with other data.
pub trait PciConfigurationSpaceIoHandler {
    type Context<'a>;
    type R;

    fn preprocess_read_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        offset: usize,
        size: usize,
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let _ = config;
        let _ = context;
        let _ = size;
        let _ = offset;
        Ok(PciHandlerResult::Unhandled)
    }

    fn postprocess_write_config(
        &mut self,
        config: &mut PciConfigurationSpace,
        offset: usize,
        size: usize,
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let _ = config;
        let _ = context;
        let _ = size;
        let _ = offset;
        Ok(PciHandlerResult::Unhandled)
    }
}
