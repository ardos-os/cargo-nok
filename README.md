# cargo-nok
<b>
<p align="center">
Build kernel modules at the speed of thought
</p>
</b>
`cargo-nok` builds out-of-tree Rust Linux kernel modules while keeping Cargo in
charge of compiling the Rust crate.

The name means **No Kbuild**: module projects do not need their own Makefile or
to invoke the usual external-module Kbuild flow. `cargo-nok` discovers the
active kernel build tree, configures Cargo and `rustc` with the kernel target
and flags, and performs the remaining ELF and module metadata steps required to
produce a loadable `.ko`.

## Status

This project is experimental. It depends on Rust-for-Linux build artifacts and
internal kernel tools whose interfaces may change between kernel releases.

Supported architectures currently include:

- x86_64
- x86 UML
- arm64
- RISC-V 64
- LoongArch 64

## How It Works

For `cargo nok build`, the tool:

1. Discovers the active Kbuild directory.
2. Verifies that the current `rustc` exactly matches the compiler used to build
   the kernel.
3. Derives the Rust target, architecture flags, kernel cfgs, and precompiled
   kernel crates from the Kbuild output.
4. Runs `cargo build` for the module's `rlib` target.
5. Links the module crate and its Rust dependencies into a relocatable object.
6. Runs the kernel's `objtool` when enabled.
7. Runs `modpost` and compiles the generated module metadata.
8. Links the final `.ko` with the kernel's module linker script.

Cargo remains responsible for dependency resolution and Rust compilation.
Kernel-provided tools are still used where the kernel module ABI requires them.

## Requirements

The running kernel must have been built with Rust support. Its Kbuild directory
must contain at least:

- `rust/libkernel.rmeta` and the other precompiled kernel Rust crates
- `include/generated/rustc_cfg`
- `include/config/auto.conf`
- `scripts/mod/modpost`
- `scripts/module.lds`
- `scripts/module-common.c`
- `tools/objtool/objtool` when `CONFIG_OBJTOOL` is enabled

The host also needs:

- Cargo and the exact `rustc` version recorded by the kernel
- a C compiler
- GNU `ld` and `objcopy`
- kernel headers/build artifacts for the target kernel
- `rust-analyzer` for editor integration

By default, the Kbuild tree is:

```text
/lib/modules/$(uname -r)/build
```

Override it with `NOK_KERNEL_DIR`.

## Installation

Install from the repository:

```bash
cargo install --git https://github.com/ardos-os/cargo-nok
```

Cargo will then expose the tool as a subcommand:

```bash
cargo nok build
```

## Module Project

The module must provide one Cargo `rlib` target:

```toml
[package]
name = "hello-kernel"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["rlib"]
```

A minimal crate root can use the normal Rust-for-Linux API:

```rust
use kernel::prelude::*;

module! {
    type: HelloKernel,
    name: "hello_kernel",
    authors: ["Example Author"],
    description: "Example out-of-tree Rust module",
    license: "GPL",
}

struct HelloKernel;

impl kernel::Module for HelloKernel {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("hello_kernel: loaded\n");
        Ok(Self)
    }
}

impl Drop for HelloKernel {
    fn drop(&mut self) {
        pr_info!("hello_kernel: unloaded\n");
    }
}
```

`cargo-nok` injects the kernel's `no_std` and nightly feature attributes during
the real compilation.

## Commands

### Build

```bash
cargo nok build
```

Cargo arguments such as `--release`, `--manifest-path`, `--package`, and
feature options are forwarded:

```bash
cargo nok build --release --package hello-kernel
```

The final `.ko` is placed beside the Cargo `rlib` artifact for the selected
profile and kernel target.

The `--target` option cannot be supplied by the user. Kernel crates such as
`core` and `kernel` are compiled for one specific kernel target, so
`cargo-nok` must control it.

### Check

```bash
cargo nok check
```

This uses the same Cargo build mode, target, kernel crates, cfgs, and rustflags
as `cargo nok build`, but stops before the ELF, `objtool`, `modpost`, and `.ko`
linking stages. It is intended for fast diagnostics and rust-analyzer.

### Expand

Requires `cargo-expand`:

```bash
cargo nok expand
```

Arguments are forwarded with the same kernel compilation environment.

### Load and Unload

```bash
cargo nok load
cargo nok unload
cargo nok reload
```

- `load` builds the module and runs `insmod`.
- `unload` runs `rmmod`.
- `reload` builds the module, unloads an existing instance, and loads the new
  module.

Privilege escalation is attempted with `sudo`, then `doas`, then `su`. These
commands operate on the running kernel and should be used carefully.

## rust-analyzer

`cargo nok ra` launches rust-analyzer through a Cargo proxy:

```bash
cargo nok ra --kernel-source /path/to/linux-source
```

The proxy:

- augments `cargo metadata` with the Linux kernel crates and their source files;
- redirects rust-analyzer's normal `cargo check` to `cargo nok check`;
- exposes the precompiled kernel proc macros;
- injects kernel cfgs, `MODULE`, `OBJTREE`, and `RUST_MODFILE`;
- preserves normal dependencies from the project's `Cargo.toml`.

The Linux source tree must match the Kbuild version exactly. It is discovered
from common locations, or can be selected explicitly:

```bash
cargo nok ra --kernel-source /usr/src/linux
```

The equivalent environment variable is:

```bash
export NOK_KERNEL_SOURCE=/usr/src/linux
```

Use a non-default rust-analyzer executable with:

```bash
cargo nok ra \
    --kernel-source /usr/src/linux \
    --rust-analyzer /path/to/rust-analyzer
```

Arguments after `--` are passed to rust-analyzer:

```bash
cargo nok ra --kernel-source /usr/src/linux -- analysis-stats .
```

### VS Code

VS Code's `rust-analyzer.server.path` accepts an executable path, not a command
with arguments. Create a launcher such as `.vscode/cargo-nok-ra`:

```sh
#!/bin/sh

exec cargo nok ra \
    --kernel-source /path/to/linux-source \
    --rust-analyzer /usr/bin/rust-analyzer \
    -- "$@"
```

Make it executable and configure `.vscode/settings.json`:

```json
{
    "rust-analyzer.server.path": "/absolute/path/to/project/.vscode/cargo-nok-ra"
}
```

No `rust-analyzer.check.overrideCommand` is required. The Cargo proxy
intercepts the default check command automatically.

## Conditional Compilation

The module crate receives:

```rust
#[cfg(MODULE)]
#[cfg(cargo_nok)]
```

`MODULE` indicates a loadable kernel module. `cargo_nok` indicates that the
crate is being compiled or analyzed through cargo-nok.

Cargo metadata cannot express rustc's unstable `-Zcrate-attr` option.
`cargo-nok` therefore exposes an editor-only cfg:

```rust
#[cfg(cargo_nok_ra)]
```

Use it when rust-analyzer needs the same crate attributes that the real build
receives through rustflags:

```rust
#![cfg_attr(cargo_nok_ra, no_std)]
#![cfg_attr(
    cargo_nok_ra,
    feature(
        asm_const,
        asm_goto,
        arbitrary_self_types,
        lint_reasons,
        offset_of_nested,
        raw_ref_op,
        slice_ptr_len,
        strict_provenance,
        used_with_arg
    )
)]
```

These attributes are active only in rust-analyzer. The real build receives the
same attributes directly from the kernel rustflags.

## Environment Variables

| Variable | Purpose |
| --- | --- |
| `NOK_KERNEL_DIR` | Override the Kbuild directory. |
| `NOK_KERNEL_SOURCE` | Select matching Linux sources for rust-analyzer. |
| `NOK_RUST_ANALYZER` | Select the rust-analyzer executable. |
| `NOK_BTF=1` | Generate BTF when `vmlinux` and the kernel helper are available. |
| `RUSTC` | Select the Rust compiler. Its version must exactly match the kernel. |
| `CARGO` | Select the Cargo executable. |
| `CC` | Select the C compiler used for module metadata. |
| `LD` | Select the linker. |
| `OBJCOPY` | Select objcopy. |

Variables prefixed with `NOK_RA_` are internal to the rust-analyzer proxy and
should not normally be set manually.

## BTF

BTF generation is opt-in:

```bash
NOK_BTF=1 cargo nok build
```

It is skipped with a warning if the Kbuild tree does not contain `vmlinux`.

## Development

Run the test suite and lints with:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

