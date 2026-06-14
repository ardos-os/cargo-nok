mod architecture;
mod config;
mod toolchain;

use std::{env, path::PathBuf, process::Command};

use crate::{Result, output};

use architecture::{Architecture, RustTarget};
use config::KernelConfig;

pub struct KernelBuild {
    pub dir: PathBuf,
    config: KernelConfig,
    architecture: Architecture,
    target: RustTarget,
}

impl KernelBuild {
    pub fn discover() -> Result<Self> {
        let dir = if let Some(path) = env::var_os("NOK_KERNEL_DIR") {
            PathBuf::from(path)
        } else {
            let output = Command::new("uname").arg("-r").output()?;
            if !output.status.success() {
                return Err("failed to determine the running kernel release".into());
            }
            let release = String::from_utf8(output.stdout)?;
            PathBuf::from("/lib/modules")
                .join(release.trim())
                .join("build")
        };

        for relative in [
            "scripts/module.lds",
            "scripts/module-common.c",
            "scripts/mod/modpost",
            "rust/libkernel.rmeta",
            "include/generated/rustc_cfg",
            "include/config/auto.conf",
        ] {
            let path = dir.join(relative);
            if !path.exists() {
                return Err(format!("kernel build artifact is missing: {}", path.display()).into());
            }
        }

        let config = KernelConfig::read(&dir.join("include/config/auto.conf"))?;
        let architecture = Architecture::detect(&config)?;
        let architecture_headers = dir
            .join("arch")
            .join(architecture.source_name())
            .join("include");
        if !architecture_headers.is_dir() {
            return Err(format!(
                "kernel architecture headers are missing: {}",
                architecture_headers.display()
            )
            .into());
        }
        validate_rustc_version(&config)?;
        let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let target = architecture.rust_target(&dir, &config);
        architecture.validate_target(&target, &rustc)?;
        if matches!(target, RustTarget::Specification(_))
            && !dir.join("scripts/target.json").is_file()
        {
            return Err("kernel target specification scripts/target.json is missing".into());
        }
        if config.enabled("CONFIG_OBJTOOL") && !dir.join("tools/objtool/objtool").is_file() {
            return Err("kernel enables objtool but its binary is missing".into());
        }
        if !dir.join("Module.symvers").is_file() {
            output::warning(
                "Module.symvers is missing; symbol versions and dependencies may be incomplete",
            );
        }

        Ok(Self {
            dir,
            config,
            architecture,
            target,
        })
    }
}

fn validate_rustc_version(config: &KernelConfig) -> Result<()> {
    let expected = config
        .value("CONFIG_RUSTC_VERSION_TEXT")
        .ok_or("kernel build does not record CONFIG_RUSTC_VERSION_TEXT")?;
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(&rustc).arg("--version").output()?;
    if !output.status.success() {
        return Err(format!(
            "failed to query Rust compiler `{}`",
            PathBuf::from(rustc).display()
        )
        .into());
    }
    let actual = String::from_utf8(output.stdout)?;
    let actual = actual.trim();
    compare_rustc_versions(expected, actual)
}

fn compare_rustc_versions(expected: &str, actual: &str) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(
            format!("Rust compiler version mismatch\n  kernel: {expected}\n  current: {actual}")
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::compare_rustc_versions;

    #[test]
    fn requires_the_exact_kernel_rustc_version() {
        let kernel = "rustc 1.96.0 (ac68faa20 2026-05-25)";
        assert!(compare_rustc_versions(kernel, kernel).is_ok());
        assert!(compare_rustc_versions(kernel, "rustc 1.96.0 (different 2026-05-25)").is_err());
        assert!(compare_rustc_versions(kernel, "rustc 1.95.0 (ac68faa20 2026-05-25)").is_err());
    }
}
