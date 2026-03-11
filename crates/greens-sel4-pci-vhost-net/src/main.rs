// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;

use argh::FromArgs;
use greens_sel4::io_interface::Sel4IoInterface;

use crate::device_model::DeviceModel;
use crate::error::Error as DeviceModelError;
use crate::guest_memory::build_guest_memory;
use crate::pci::{InterruptController, VhostNetPci};
use crate::vhost_net::{VhostNetConfig, VhostNetDevice};

mod device_model;
mod error;
mod guest_memory;
mod pci;
mod tap;
mod vhost_net;

const CLI_NAME: &str = env!("CARGO_PKG_NAME");
const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
const SEL4_DEV_PATH: &str = "/dev/sel4";

/// vhost-net frontend application providing virtio-net device
#[derive(FromArgs, Debug)]
struct VhostNetCli {
    /// backend device model id
    #[argh(positional)]
    device_model_id: u64,

    /// guest mac address
    #[argh(option, short = 'm', from_str_fn(parse_mac_address))]
    mac: Option<[u8; 6]>,

    /// name of the TAP device (default: tap0)
    #[argh(option, short = 't', default = "String::from(\"tap0\")")]
    tap: String,

    /// virtqueue size
    #[argh(option, short = 's', from_str_fn(parse_virtqueue_size))]
    queue_size: Option<u16>,

    /// application version
    #[argh(switch, short = 'v')]
    version: bool,
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli: VhostNetCli = argh::from_env();

    if cli.version {
        println!("{CLI_NAME} {CLI_VERSION}");
        return Ok(());
    }

    let config = VhostNetConfig {
        mac: cli.mac,
        tap: cli.tap,
        queue_size: cli.queue_size.unwrap_or(256),
    };

    let vmio = Sel4IoInterface::new(SEL4_DEV_PATH.into(), cli.device_model_id)
        .map_err(DeviceModelError::Init)?;
    let vmio = Arc::new(vmio);

    vmio.register_vpci_device()
        .map_err(DeviceModelError::Init)?;
    let guest_memory = build_guest_memory(vmio.guest_memory())?;

    let mut model = DeviceModel::new(vmio.clone(), vmio.clone(), guest_memory, config);

    model.run()?;

    Ok(())
}

fn main() -> ExitCode {
    if let Err(e) = run() {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn parse_mac_address(arg: &str) -> Result<[u8; 6], String> {
    let mut mac = vec![];
    for octet in arg.split(":") {
        mac.push(u8::from_str_radix(octet, 16).map_err(|e| format!("parsing octet failed: {e}"))?);
    }

    Ok(mac.try_into().map_err(|_| "incorrect number of octets")?)
}

fn parse_virtqueue_size(arg: &str) -> Result<u16, String> {
    let size = arg
        .parse::<u16>()
        .map_err(|e| format!("parsing virtqueue size failed: {e}"))?;

    // Must be power of two
    if !size.is_power_of_two() {
        return Err("provided virtqueue size is not power of two".into());
    }

    // Validate range
    if size < 2 || size > 2u16.pow(15) {
        return Err("provided virtqueue size out of range".into());
    }

    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mac() {
        assert_eq!(
            parse_mac_address("aa:bb:cc:dd:ee:ff"),
            Ok([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
        );
    }

    #[test]
    fn test_parse_mac_invalid_length() {
        assert!(parse_mac_address("aa:bb:cc:dd:ee").is_err());
        assert!(parse_mac_address("aa:bb:cc:dd:ee:ff:00").is_err());
    }

    #[test]
    fn test_parse_mac_invalid_octet() {
        assert!(parse_mac_address("bl:bb:cc:dd:ee:ff").is_err());
        assert!(parse_mac_address("11:bb:101:dd:ee:ff").is_err());
        assert!(parse_mac_address("11:bb:1:dd:ee:").is_err());
    }

    #[test]
    fn test_parse_mac_malformed() {
        assert!(parse_mac_address("blah").is_err());
    }

    #[test]
    fn test_parse_virtqueue_size() {
        for i in 1..16 {
            let s = 2u16.pow(i);
            assert_eq!(parse_virtqueue_size(&s.to_string()), Ok(s));
        }
    }

    #[test]
    fn test_parse_virtqueue_size_non_pot() {
        // The virtqueue size must be power of two
        assert!(parse_virtqueue_size("5").is_err());
        assert!(parse_virtqueue_size("3").is_err());
        assert!(parse_virtqueue_size("32767").is_err());
    }

    #[test]
    fn test_parse_virtqueue_size_invalid() {
        assert!(parse_virtqueue_size("-1").is_err());
        assert!(parse_virtqueue_size("0").is_err());
    }

    #[test]
    fn test_parse_virtqueue_size_limits() {
        assert!(parse_virtqueue_size("1").is_err());
        assert_eq!(parse_virtqueue_size("2"), Ok(2));

        // Max size is 32768
        assert!(parse_virtqueue_size("65536").is_err());
    }

    #[test]
    fn test_parse_virtqueue_size_nan() {
        assert!(parse_virtqueue_size("blah").is_err());
    }
}
