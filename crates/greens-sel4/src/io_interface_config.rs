// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::collections::HashMap;
use std::fs;
use std::io;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("failed to parse io interface parameters")]
    KernelCmdline(io::Error),
    #[error("failed to parse io interface parameters")]
    ParseIoInterface,
    #[error("no io interface config for {0}")]
    NoIoInterfaceConfig(u64),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct IoInterfaceConfig {
    pub ram_start: u64,
    pub ram_size: u64,
    pub pci_start: u64,
    pub pci_size: u64,
}

impl IoInterfaceConfig {
    fn from_iter(mut iter: impl Iterator<Item = u64>) -> Option<IoInterfaceConfig> {
        Some(IoInterfaceConfig {
            ram_start: iter.next()?,
            ram_size: iter.next()?,
            #[allow(dead_code)]
            pci_start: iter.next()?,
            pci_size: iter.next()?,
        })
    }
}

impl TryFrom<&str> for IoInterfaceConfig {
    type Error = Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        let mut values = Vec::new();
        let iter = value
            .split(',')
            .map(|v| parse_u64(v).map_err(|_| Error::ParseIoInterface));
        for item in iter {
            values.push(item?);
        }

        Self::from_iter(values.into_iter()).ok_or(Error::ParseIoInterface)
    }
}

fn parse_u64(s: &str) -> std::result::Result<u64, std::num::ParseIntError> {
    if let Some(s) = s.strip_prefix("0x") {
        u64::from_str_radix(s, 16)
    } else {
        s.parse::<u64>()
    }
}

fn read_kernel_cmdline() -> Result<String, Error> {
    fs::read_to_string("/proc/cmdline").map_err(Error::KernelCmdline)
}

fn kernel_param(name: &str) -> Result<Vec<String>, Error> {
    let cmdline = read_kernel_cmdline()?;

    let params: Vec<String> = cmdline
        .split_whitespace()
        .filter(|s| s.starts_with(name))
        .map(|s| s.to_string())
        .collect();

    Ok(params)
}

pub fn parse_io_interface_configs(
    args: Vec<String>,
) -> Result<HashMap<u64, IoInterfaceConfig>, Error> {
    let mut configs: HashMap<u64, IoInterfaceConfig> = HashMap::new();

    for arg in args {
        let mut config = arg
            .strip_prefix("uservm=")
            .ok_or(Error::ParseIoInterface)?
            .splitn(2, ',');

        let id = config
            .next()
            .and_then(|v| parse_u64(v).ok())
            .ok_or(Error::ParseIoInterface)?;
        let cfg = config
            .next()
            .and_then(|v| IoInterfaceConfig::try_from(v).ok())
            .ok_or(Error::ParseIoInterface)?;

        configs.insert(id, cfg);
    }

    Ok(configs)
}

pub(crate) fn io_interface_config(device_model_id: u64) -> Result<IoInterfaceConfig, Error> {
    // E.g. uservm=1,0x60000000,0x2000000,0xc0000000,0x80000
    let params = kernel_param("uservm")?;

    let configs = parse_io_interface_configs(params)?;
    configs
        .get(&device_model_id)
        .ok_or(Error::NoIoInterfaceConfig(device_model_id))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_cfg(cfg: &IoInterfaceConfig, ram: u64, ram_size: u64, pci: u64, pci_size: u64) {
        assert_eq!(cfg.ram_start, ram);
        assert_eq!(cfg.ram_size, ram_size);
        assert_eq!(cfg.pci_start, pci);
        assert_eq!(cfg.pci_size, pci_size);
    }

    #[test]
    fn test_parse_io_interfaces_one() {
        let args = vec!["uservm=1,0x60000000,0x2000000,0xc0000000,0x80000".into()];
        let configs = parse_io_interface_configs(args).expect("parse");
        let cfg = configs.get(&1).expect("get");
        check_cfg(&cfg, 0x60000000, 0x2000000, 0xc0000000, 0x80000);
    }

    #[test]
    fn test_parse_io_interfaces_many() {
        let args = vec![
            "uservm=1,0x10000000,0x2000000,0x20000000,0x80000",
            "uservm=2,0x30000000,0x2000000,0x40000000,0x80000",
            "uservm=3,0x50000000,0x2000000,0x60000000,0x80000",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let configs = parse_io_interface_configs(args).expect("parse");
        let cfg = configs.get(&1).expect("get");
        check_cfg(&cfg, 0x10000000, 0x2000000, 0x20000000, 0x80000);
        let cfg = configs.get(&2).expect("get");
        check_cfg(&cfg, 0x30000000, 0x2000000, 0x40000000, 0x80000);
        let cfg = configs.get(&3).expect("get");
        check_cfg(&cfg, 0x50000000, 0x2000000, 0x60000000, 0x80000);
    }

    #[test]
    fn test_parse_io_interfaces_empty() {
        let args = vec![];
        let configs = parse_io_interface_configs(args).expect("parse");
        assert_eq!(configs.keys().len(), 0);
    }

    #[test]
    fn test_parse_io_interfaces_malformed() {
        let args = vec!["uservm=1,0x10000000,0x2000000,0x20000000,x80000".into()];
        assert!(parse_io_interface_configs(args).is_err());
        let args = vec!["uservm=,0x10000000,0x2000000,0x20000000,0x80000".into()];
        assert!(parse_io_interface_configs(args).is_err());
        let args = vec!["uservm=1,0x10000000,0x20000000,0x80000".into()];
        assert!(parse_io_interface_configs(args).is_err());
        let args = vec!["1,0x10000000,0x2000000,0x20000000,0x80000".into()];
        assert!(parse_io_interface_configs(args).is_err());
    }
}
