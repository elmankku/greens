// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
use std::env;
use std::path::PathBuf;

use bindgen::callbacks::{DeriveInfo, ParseCallbacks};

#[derive(Debug)]
struct ExtraDeriveCallback {
    names: Vec<String>,
    derives: Vec<String>,
}

impl ExtraDeriveCallback {
    fn new(name: &str, derives: &str) -> Self {
        Self {
            names: vec![name.to_owned()],
            derives: derives.split(',').map(|s| s.to_owned()).collect(),
        }
    }
}

impl ParseCallbacks for ExtraDeriveCallback {
    fn add_derives(&self, info: &DeriveInfo<'_>) -> Vec<String> {
        if self.names.iter().any(|n| n == info.name) {
            return self.derives.clone();
        }

        vec![]
    }
}

fn build_libextern(include_path: Option<String>) {
    cc::Build::new()
        .file(std::env::temp_dir().join("bindgen").join("extern.c"))
        .extra_warnings(true)
        .warnings(true)
        .include(".")
        .includes(include_path)
        .compile("libextern.a");
}

fn main() {
    let input = "wrapper.h";
    let include_path = env::var("SEL4_VIRT_HEADERS_PATH");
    let include_flag = format!("-I{}", include_path.to_owned().unwrap_or(String::from("")));

    let mut builder = bindgen::Builder::default()
        .header(input)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .clang_arg(include_flag)
        .wrap_static_fns(true)
        .disable_header_comment()
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(true)
        .size_t_is_usize(true)
        .allowlist_file(".*sel4_virt.*.h")
        .allowlist_file(".*rpc.*.h")
        .allowlist_file(input)
        .blocklist_item("__kernel.*")
        .blocklist_item("__BITS_PER_LONG")
        .blocklist_item("__FD_SETSIZE")
        .blocklist_item("_?IOC.*")
        .raw_line("use zerocopy::AsBytes;")
        .raw_line("use zerocopy::FromBytes;")
        .raw_line("use zerocopy::FromZeroes;");

    // Add extra derives for certain types
    let extra_derives = "FromZeroes,FromBytes,AsBytes";
    let derives_for = ["rpcmsg_t"];

    for regex in derives_for {
        builder = builder.parse_callbacks(Box::new(ExtraDeriveCallback::new(regex, extra_derives)));
    }

    let bindings = builder.generate().expect("Unable to generate bindings");

    // compile the generated wrappers into a static library
    build_libextern(include_path.ok());

    // rebuild on wrapper changes
    println!("cargo:rerun-if-changed={}", input);

    // statically link against `libextern`
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!(
        "cargo:rustc-link-search=native={}",
        out_path.to_string_lossy()
    );
    println!("cargo:rustc-link-lib=static=extern");

    bindings
        .write_to_file(out_path.join("sel4_virt_generated.rs"))
        .expect("Couldn't write bindings!");
}
