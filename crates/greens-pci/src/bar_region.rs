// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use crate::bar::PciBarIndex;
use crate::function::PciHandlerResult;
use crate::utils::range_contains;
use crate::Result;

#[derive(Debug, Clone)]
pub struct PciBarRegionInfo {
    pub bar: PciBarIndex,
    pub offset: u64,
    pub length: u64,
}

impl PciBarRegionInfo {
    pub fn new(bar: PciBarIndex, offset: u64, length: u64) -> Self {
        Self {
            bar,
            offset,
            length,
        }
    }

    pub fn access_targets_region(&self, bar: PciBarIndex, offset: u64, size: usize) -> bool {
        self.bar == bar && range_contains(self.offset, self.length, offset, size as u64)
    }

    pub fn offset_within_region(&self, bar: PciBarIndex, offset: u64, size: usize) -> Option<u64> {
        match self.access_targets_region(bar, offset, size) {
            true => Some(offset - self.offset),
            false => None,
        }
    }
}

pub trait PciBarRegion {
    fn info(&self) -> &PciBarRegionInfo;
}

pub trait SimplePciBarRegionHandler: PciBarRegion {
    type R;

    fn handle_read_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
    ) -> Result<PciHandlerResult<Self::R>> {
        let Some(offset) = self.info().offset_within_region(bar, offset, data.len()) else {
            return Ok(PciHandlerResult::Unhandled);
        };

        Ok(PciHandlerResult::Handled(self.read_bar(offset, data)?))
    }

    fn handle_write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
    ) -> Result<PciHandlerResult<Self::R>> {
        let Some(offset) = self.info().offset_within_region(bar, offset, data.len()) else {
            return Ok(PciHandlerResult::Unhandled);
        };

        Ok(PciHandlerResult::Handled(self.write_bar(offset, data)?))
    }

    fn read_bar(&mut self, offset: u64, data: &mut [u8]) -> Result<Self::R>;
    fn write_bar(&mut self, offset: u64, data: &[u8]) -> Result<Self::R>;
}

pub trait PciBarRegionHandler: PciBarRegion {
    type Context<'a>;
    type R;

    fn handle_read_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let Some(offset) = self.info().offset_within_region(bar, offset, data.len()) else {
            return Ok(PciHandlerResult::Unhandled);
        };

        let r = self.read_bar(offset, data, context)?;
        Ok(PciHandlerResult::Handled(r))
    }

    fn handle_write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let Some(offset) = self.info().offset_within_region(bar, offset, data.len()) else {
            return Ok(PciHandlerResult::Unhandled);
        };

        let r = self.write_bar(offset, data, context)?;
        Ok(PciHandlerResult::Handled(r))
    }

    fn read_bar(
        &mut self,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R>;

    fn write_bar(
        &mut self,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R>;
}

pub trait PciBarRegionSet {
    type RegionId;

    fn info_iter(&self) -> impl Iterator<Item = (Self::RegionId, &PciBarRegionInfo)>;
}

pub trait PciBarRegionSetHandler: PciBarRegionSet {
    type Context<'a>;
    type R;

    fn handle_read_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let region = self
            .info_iter()
            .find(|(_, info)| info.offset_within_region(bar, offset, data.len()).is_some());

        match region {
            Some((id, info)) => {
                // SAFETY: checked by find above
                let offset = info.offset_within_region(bar, offset, data.len()).unwrap();
                let r = self.read_bar(id, offset, data, context)?;
                Ok(PciHandlerResult::Handled(r))
            }
            None => Ok(PciHandlerResult::Unhandled),
        }
    }

    fn handle_write_bar(
        &mut self,
        bar: PciBarIndex,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<PciHandlerResult<Self::R>> {
        let region = self
            .info_iter()
            .find(|(_, info)| info.offset_within_region(bar, offset, data.len()).is_some());

        match region {
            Some((id, info)) => {
                // SAFETY: checked by find above
                let offset = info.offset_within_region(bar, offset, data.len()).unwrap();
                let r = self.write_bar(id, offset, data, context)?;
                Ok(PciHandlerResult::Handled(r))
            }
            None => Ok(PciHandlerResult::Unhandled),
        }
    }

    fn read_bar(
        &mut self,
        id: Self::RegionId,
        offset: u64,
        data: &mut [u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R>;

    fn write_bar(
        &mut self,
        id: Self::RegionId,
        offset: u64,
        data: &[u8],
        context: &mut Self::Context<'_>,
    ) -> Result<Self::R>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHandler {
        infos: [PciBarRegionInfo; 2],
        last_offset: u64,
    }

    use PciBarRegionHandler as Normal;
    use PciBarRegionSetHandler as Set;
    use SimplePciBarRegionHandler as Simple;

    impl TestHandler {
        fn new() -> Self {
            let infos = [
                PciBarRegionInfo::new(PciBarIndex::default(), 8, 8),
                PciBarRegionInfo::new(PciBarIndex::default(), 16, 8),
            ];

            Self {
                infos,
                last_offset: u64::max_value(),
            }
        }
    }

    impl PciBarRegion for TestHandler {
        fn info(&self) -> &PciBarRegionInfo {
            &self.infos[0]
        }
    }

    impl PciBarRegionSet for TestHandler {
        type RegionId = usize;
        fn info_iter(&self) -> impl Iterator<Item = (Self::RegionId, &PciBarRegionInfo)> {
            self.infos.iter().enumerate()
        }
    }

    impl Simple for TestHandler {
        type R = ();

        fn read_bar(&mut self, offset: u64, _data: &mut [u8]) -> Result<Self::R> {
            self.last_offset = offset;
            Ok(())
        }

        fn write_bar(&mut self, offset: u64, _data: &[u8]) -> Result<Self::R> {
            self.last_offset = offset;
            Ok(())
        }
    }

    impl Normal for TestHandler {
        type Context<'a> = u32;
        type R = ();

        fn read_bar(
            &mut self,
            offset: u64,
            _data: &mut [u8],
            context: &mut Self::Context<'_>,
        ) -> Result<Self::R> {
            self.last_offset = offset;
            *context = 1;
            Ok(())
        }

        fn write_bar(
            &mut self,
            offset: u64,
            _data: &[u8],
            context: &mut Self::Context<'_>,
        ) -> Result<Self::R> {
            self.last_offset = offset;
            *context = 2;
            Ok(())
        }
    }

    impl Set for TestHandler {
        type Context<'a> = u32;
        type R = ();

        fn read_bar(
            &mut self,
            id: Self::RegionId,
            offset: u64,
            _data: &mut [u8],
            context: &mut Self::Context<'_>,
        ) -> Result<Self::R> {
            self.last_offset = offset;

            match id {
                0 => *context = 3,
                1 => *context = 4,
                _ => unreachable!("This is a BUG!"),
            }

            Ok(())
        }

        fn write_bar(
            &mut self,
            id: Self::RegionId,
            offset: u64,
            _data: &[u8],
            context: &mut Self::Context<'_>,
        ) -> Result<Self::R> {
            self.last_offset = offset;

            match id {
                0 => *context = 5,
                1 => *context = 6,
                _ => unreachable!("This is a BUG!"),
            }

            Ok(())
        }
    }

    #[test]
    fn test_handle_read_bar_no_match() {
        let mut h = TestHandler::new();

        // Does not handle reads to non-matching BARs
        let index = PciBarIndex::try_from(1).unwrap();
        let r = Simple::handle_read_bar(&mut h, index, 8, &mut [0x00u8; 4]).unwrap();
        assert!(matches!(r, PciHandlerResult::Unhandled));

        // Does not handle reads to non-matching regions
        let index = PciBarIndex::default();
        let r = Simple::handle_read_bar(&mut h, index, 0, &mut [0x00u8; 4]).unwrap();
        assert!(matches!(r, PciHandlerResult::Unhandled));

        // Does not handle reads to partially mathing regions
        let r = Simple::handle_read_bar(&mut h, index, 12, &mut [0x00u8; 8]).unwrap();
        assert!(matches!(r, PciHandlerResult::Unhandled));
    }

    #[test]
    fn test_handle_read_bar() {
        let mut h = TestHandler::new();
        let mut data = [0x00u8; 4];
        let index = PciBarIndex::default();

        let r = Simple::handle_read_bar(&mut h, index, 8, &mut data).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 0);

        let r = Simple::handle_read_bar(&mut h, index, 12, &mut data).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 4);
    }

    #[test]
    fn test_handle_write_bar_no_match() {
        let mut h = TestHandler::new();

        // Does not handle writes to non-matching BARs
        let index = PciBarIndex::try_from(1).unwrap();
        let r = Simple::handle_write_bar(&mut h, index, 8, &[0x00u8; 4]).unwrap();
        assert!(matches!(r, PciHandlerResult::Unhandled));

        // Does not handle writes to non-matching regions
        let index = PciBarIndex::default();
        let r = Simple::handle_write_bar(&mut h, index, 0, &[0x00u8; 4]).unwrap();
        assert!(matches!(r, PciHandlerResult::Unhandled));

        // Does not handle writes to partially mathing regions
        let r = Simple::handle_write_bar(&mut h, PciBarIndex::default(), 12, &[0x00u8; 8]).unwrap();
        assert!(matches!(r, PciHandlerResult::Unhandled));
    }

    #[test]
    fn test_handle_write_bar() {
        let mut h = TestHandler::new();
        let data = [0x00u8; 4];
        let index = PciBarIndex::default();

        let r = Simple::handle_write_bar(&mut h, index, 8, &data).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 0);

        let r = Simple::handle_write_bar(&mut h, index, 12, &data).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 4);
    }

    #[test]
    fn test_handle_read_bar_with_context() {
        let mut h = TestHandler::new();
        let mut data = [0x00u8; 4];
        let mut context = 0;
        let index = PciBarIndex::default();

        let r = Normal::handle_read_bar(&mut h, index, 8, &mut data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 0);
        assert_eq!(context, 1);

        let r = Normal::handle_read_bar(&mut h, index, 12, &mut data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 4);
        assert_eq!(context, 1);
    }

    #[test]
    fn test_handle_write_bar_with_context() {
        let mut h = TestHandler::new();
        let data = [0x00u8; 4];
        let mut context = 0;
        let index = PciBarIndex::default();

        let r = Normal::handle_write_bar(&mut h, index, 8, &data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 0);
        assert_eq!(context, 2);

        let r = Normal::handle_write_bar(&mut h, index, 12, &data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));

        assert_eq!(h.last_offset, 4);
        assert_eq!(context, 2);
    }

    #[test]
    fn test_handle_read_bar_multiple_regions() {
        let mut h = TestHandler::new();
        let mut data = [0x00u8; 4];
        let mut context = 0;
        let index = PciBarIndex::default();

        let r = Set::handle_read_bar(&mut h, index, 0, &mut data, &mut context).unwrap();
        assert!(!r.handled());

        let r = Set::handle_read_bar(&mut h, index, 12, &mut data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 4);
        assert_eq!(context, 3);

        let r = Set::handle_read_bar(&mut h, index, 16, &mut data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 0);
        assert_eq!(context, 4);
    }

    #[test]
    fn test_handle_write_bar_multiple_regions() {
        let mut h = TestHandler::new();
        let data = [0x00u8; 4];
        let mut context = 0;
        let index = PciBarIndex::default();

        let r = Set::handle_write_bar(&mut h, index, 12, &data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 4);
        assert_eq!(context, 5);

        let r = Set::handle_write_bar(&mut h, index, 16, &data, &mut context).unwrap();
        assert!(matches!(r, PciHandlerResult::Handled(_)));
        assert_eq!(h.last_offset, 0);
        assert_eq!(context, 6);
    }
}
