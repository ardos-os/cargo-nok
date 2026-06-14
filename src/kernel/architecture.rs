use std::{
    ffi::{OsStr, OsString},
    path::Path,
    process::Command,
};

use crate::Result;

use super::config::KernelConfig;

pub enum RustTarget {
    BuiltIn(&'static str),
    Specification(OsString),
}

impl RustTarget {
    pub fn as_os_str(&self) -> &OsStr {
        match self {
            Self::BuiltIn(target) => OsStr::new(target),
            Self::Specification(path) => path,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Architecture {
    X86_64,
    X86Uml32,
    Arm64,
    Riscv64,
    LoongArch64,
}

impl Architecture {
    pub fn detect(config: &KernelConfig) -> Result<Self> {
        if config.enabled("CONFIG_X86_64") {
            Ok(Self::X86_64)
        } else if config.enabled("CONFIG_X86_32") && config.enabled("CONFIG_UML") {
            Ok(Self::X86Uml32)
        } else if config.enabled("CONFIG_ARM64") {
            Ok(Self::Arm64)
        } else if config.enabled("CONFIG_RISCV") && config.enabled("CONFIG_64BIT") {
            Ok(Self::Riscv64)
        } else if config.enabled("CONFIG_LOONGARCH") && config.enabled("CONFIG_64BIT") {
            Ok(Self::LoongArch64)
        } else {
            Err("the selected kernel architecture is not supported by Rust-for-Linux".into())
        }
    }

    pub fn source_name(self) -> &'static str {
        match self {
            Self::X86_64 | Self::X86Uml32 => "x86",
            Self::Arm64 => "arm64",
            Self::Riscv64 => "riscv",
            Self::LoongArch64 => "loongarch",
        }
    }

    pub fn rust_target(self, kernel_dir: &Path, config: &KernelConfig) -> RustTarget {
        match self {
            Self::X86_64 | Self::X86Uml32 => {
                RustTarget::Specification(kernel_dir.join("scripts/target.json").into_os_string())
            }
            Self::Arm64 if config.version_at_least("CONFIG_RUSTC_VERSION", 1, 85, 0) => {
                RustTarget::BuiltIn("aarch64-unknown-none-softfloat")
            }
            Self::Arm64 => RustTarget::BuiltIn("aarch64-unknown-none"),
            Self::Riscv64 => RustTarget::BuiltIn("riscv64imac-unknown-none-elf"),
            Self::LoongArch64 => RustTarget::BuiltIn("loongarch64-unknown-none-softfloat"),
        }
    }

    pub fn rust_flags(self, config: &KernelConfig) -> Vec<String> {
        match self {
            Self::X86_64 => x86_64_rust_flags(config),
            Self::X86Uml32 => {
                vec!["-Ctarget-feature=-sse,-sse2,-sse3,-ssse3,-sse4.1,-sse4.2,-avx,-avx2".into()]
            }
            Self::Arm64 => arm64_rust_flags(config),
            Self::Riscv64 => {
                let mut flags = vec!["-Ctarget-cpu=generic-rv64".into(), "-Cno-redzone=y".into()];
                if !config.enabled("CONFIG_RISCV_ISA_C") {
                    flags.push("-Ctarget-feature=-c".into());
                }
                flags
            }
            Self::LoongArch64 => loongarch64_rust_flags(config),
        }
    }

    pub fn c_flags(self, config: &KernelConfig) -> Vec<String> {
        match self {
            Self::X86_64 => x86_64_c_flags(config),
            Self::X86Uml32 => vec!["-m32", "-msoft-float", "-mregparm=3", "-freg-struct-return"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Self::Arm64 => arm64_c_flags(config),
            Self::Riscv64 => {
                let mut march = String::from("rv64ima");
                if config.enabled("CONFIG_RISCV_ISA_C") {
                    march.push('c');
                }
                let mut flags = vec![
                    "-mabi=lp64".into(),
                    format!("-march={march}"),
                    "-mno-save-restore".into(),
                    "-fno-unwind-tables".into(),
                ];
                if config.enabled("CONFIG_CMODEL_MEDLOW") {
                    flags.push("-mcmodel=medlow".into());
                }
                if config.enabled("CONFIG_CMODEL_MEDANY") {
                    flags.push("-mcmodel=medany".into());
                }
                flags
            }
            Self::LoongArch64 => loongarch64_c_flags(config),
        }
    }

    pub fn validate_target(self, target: &RustTarget, rustc: &OsStr) -> Result<()> {
        let RustTarget::BuiltIn(expected) = target else {
            return Ok(());
        };
        let output = Command::new(rustc)
            .args(["--print", "target-list"])
            .output()?;
        if !output.status.success() {
            return Err("failed to query Rust compiler target list".into());
        }
        let targets = String::from_utf8(output.stdout)?;
        if targets.lines().any(|target| target == *expected) {
            Ok(())
        } else {
            Err(format!(
                "Rust compiler does not provide the kernel target `{expected}` for {self:?}"
            )
            .into())
        }
    }
}

fn x86_64_c_flags(config: &KernelConfig) -> Vec<String> {
    let mut flags: Vec<String> = vec![
        "-mno-sse",
        "-mno-mmx",
        "-mno-sse2",
        "-mno-3dnow",
        "-mno-avx",
        "-mno-sse4a",
        "-mno-avx2",
        "-fno-tree-vectorize",
        "-m64",
        "-mno-80387",
        "-mno-fp-ret-in-387",
        "-mpreferred-stack-boundary=3",
        "-march=x86-64",
        "-mtune=generic",
        "-mno-red-zone",
        "-mcmodel=kernel",
        "-mindirect-branch=thunk-extern",
        "-mindirect-branch-register",
        "-mindirect-branch-cs-prefix",
        "-mfunction-return=thunk-extern",
        "-mharden-sls=all",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if config.enabled("CONFIG_X86_KERNEL_IBT") {
        flags.push("-fcf-protection=branch".into());
        flags.push("-fno-jump-tables".into());
    }
    if config.enabled("CONFIG_PREFIX_SYMBOLS")
        && let Some(bytes) = config.value("CONFIG_FUNCTION_PADDING_BYTES")
    {
        flags.push(format!("-fpatchable-function-entry={bytes},{bytes}"));
    }
    flags
}

fn x86_64_rust_flags(config: &KernelConfig) -> Vec<String> {
    let mut flags = vec![
        "-Ctarget-feature=-sse,-sse2,-sse3,-ssse3,-sse4.1,-sse4.2,-avx,-avx2".into(),
        "-Ctarget-cpu=x86-64".into(),
        "-Ztune-cpu=generic".into(),
        "-Cno-redzone=y".into(),
        "-Ccode-model=kernel".into(),
    ];
    if config.enabled("CONFIG_X86_KERNEL_IBT") {
        flags.push("-Zcf-protection=branch".into());
        flags.push("-Cjump-tables=n".into());
    }
    if config.enabled("CONFIG_MITIGATION_RETHUNK") {
        flags.push("-Zfunction-return=thunk-extern".into());
    }
    if config.enabled("CONFIG_PREFIX_SYMBOLS")
        && let Some(bytes) = config.value("CONFIG_FUNCTION_PADDING_BYTES")
    {
        flags.push(format!("-Zpatchable-function-entry={bytes},{bytes}"));
    }
    flags
}

fn arm64_rust_flags(config: &KernelConfig) -> Vec<String> {
    let mut flags = Vec::new();
    if !config.version_at_least("CONFIG_RUSTC_VERSION", 1, 85, 0) {
        flags.push("-Ctarget-feature=-neon".into());
    }
    if config.enabled("CONFIG_UNWIND_TABLES") {
        flags.push("-Cforce-unwind-tables=y".into());
        flags.push("-Zuse-sync-unwind=n".into());
    }
    if config.enabled("CONFIG_ARM64_BTI_KERNEL") {
        flags.push("-Zbranch-protection=bti,pac-ret".into());
    } else if config.enabled("CONFIG_ARM64_PTR_AUTH_KERNEL") {
        flags.push("-Zbranch-protection=pac-ret".into());
    }
    if config.enabled("CONFIG_SHADOW_CALL_STACK") {
        flags.push("-Zfixed-x18".into());
    }
    flags
}

fn arm64_c_flags(config: &KernelConfig) -> Vec<String> {
    let mut flags = vec!["-mgeneral-regs-only".into(), "-mabi=lp64".into()];
    flags.push(
        if config.enabled("CONFIG_CPU_BIG_ENDIAN") {
            "-mbig-endian"
        } else {
            "-mlittle-endian"
        }
        .into(),
    );
    if config.enabled("CONFIG_UNWIND_TABLES") {
        flags.push("-fasynchronous-unwind-tables".into());
    } else {
        flags.push("-fno-unwind-tables".into());
    }
    if config.enabled("CONFIG_ARM64_BTI_KERNEL") {
        flags.push("-mbranch-protection=pac-ret+bti".into());
    } else if config.enabled("CONFIG_ARM64_PTR_AUTH_KERNEL") {
        if config.enabled("CONFIG_CC_HAS_BRANCH_PROT_PAC_RET") {
            flags.push("-mbranch-protection=pac-ret".into());
        } else {
            flags.push("-msign-return-address=non-leaf".into());
        }
    }
    if config.enabled("CONFIG_SHADOW_CALL_STACK") {
        flags.push("-ffixed-x18".into());
    }
    flags
}

fn loongarch64_rust_flags(config: &KernelConfig) -> Vec<String> {
    let mut flags = vec![
        "-Ccode-model=small".into(),
        "-Zdirect-access-external-data=no".into(),
    ];
    if config.enabled("CONFIG_OBJTOOL") {
        if config.enabled("CONFIG_RUSTC_HAS_ANNOTATE_TABLEJUMP") {
            flags.push("-Cllvm-args=--loongarch-annotate-tablejump".into());
        } else if config.version_at_least("CONFIG_RUSTC_VERSION", 1, 93, 0) {
            flags.push("-Cjump-tables=n".into());
        } else {
            flags.push("-Zno-jump-tables".into());
        }
    }
    flags
}

fn loongarch64_c_flags(config: &KernelConfig) -> Vec<String> {
    let mut flags: Vec<String> = vec![
        "-m64",
        "-march=loongarch64",
        "-mabi=lp64s",
        "-mcmodel=normal",
        "-pipe",
        "-msoft-float",
        "-mno-relax",
        "-fno-direct-access-external-data",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if config.enabled("CONFIG_AS_HAS_EXPLICIT_RELOCS") {
        flags.push("-mexplicit-relocs".into());
    } else {
        flags.extend(
            [
                "-mno-explicit-relocs",
                "-fplt",
                "-Wa,-mla-global-with-abs,-mla-local-with-abs",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    if config.enabled("CONFIG_OBJTOOL") && !config.enabled("CONFIG_CC_HAS_ANNOTATE_TABLEJUMP") {
        flags.push("-fno-jump-tables".into());
    }
    if config.enabled("CONFIG_ARCH_STRICT_ALIGN") {
        flags.push("-mstrict-align".into());
    } else {
        flags.push("-mno-strict-align".into());
    }
    if !config.enabled("CONFIG_KASAN") {
        flags.extend(
            [
                "-fno-builtin-memcpy",
                "-fno-builtin-memmove",
                "-fno-builtin-memset",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    flags
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(values: &[(&str, &str)]) -> KernelConfig {
        KernelConfig::from_values(HashMap::from_iter(
            values
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into())),
        ))
    }

    #[test]
    fn detects_supported_architectures() {
        assert_eq!(
            Architecture::detect(&config(&[("CONFIG_ARM64", "y")])).unwrap(),
            Architecture::Arm64
        );
        assert_eq!(
            Architecture::detect(&config(&[("CONFIG_RISCV", "y"), ("CONFIG_64BIT", "y")])).unwrap(),
            Architecture::Riscv64
        );
    }

    #[test]
    fn selects_builtin_targets_for_non_x86_architectures() {
        let arm64 = config(&[("CONFIG_ARM64", "y"), ("CONFIG_RUSTC_VERSION", "109600")]);
        assert!(matches!(
            Architecture::Arm64.rust_target(Path::new("/kernel"), &arm64),
            RustTarget::BuiltIn("aarch64-unknown-none-softfloat")
        ));
        assert!(
            !Architecture::Arm64
                .rust_flags(&arm64)
                .iter()
                .any(|flag| flag.contains("-neon"))
        );

        let old_arm64 = config(&[("CONFIG_ARM64", "y"), ("CONFIG_RUSTC_VERSION", "108400")]);
        assert!(
            Architecture::Arm64
                .rust_flags(&old_arm64)
                .contains(&"-Ctarget-feature=-neon".into())
        );

        let riscv = config(&[("CONFIG_RISCV", "y"), ("CONFIG_64BIT", "y")]);
        assert!(matches!(
            Architecture::Riscv64.rust_target(Path::new("/kernel"), &riscv),
            RustTarget::BuiltIn("riscv64imac-unknown-none-elf")
        ));
    }

    #[test]
    fn maps_rust_architecture_to_kernel_source_architecture() {
        assert_eq!(Architecture::X86_64.source_name(), "x86");
        assert_eq!(Architecture::Arm64.source_name(), "arm64");
        assert_eq!(Architecture::Riscv64.source_name(), "riscv");
        assert_eq!(Architecture::LoongArch64.source_name(), "loongarch");
    }

    #[test]
    fn derives_loongarch_objtool_flags_from_kernel_config() {
        let config = config(&[
            ("CONFIG_LOONGARCH", "y"),
            ("CONFIG_64BIT", "y"),
            ("CONFIG_OBJTOOL", "y"),
            ("CONFIG_RUSTC_VERSION", "109600"),
        ]);
        assert!(
            Architecture::LoongArch64
                .rust_flags(&config)
                .contains(&"-Cjump-tables=n".into())
        );
        assert!(
            Architecture::LoongArch64
                .c_flags(&config)
                .contains(&"-fno-jump-tables".into())
        );
    }
}
