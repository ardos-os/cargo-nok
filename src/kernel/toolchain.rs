use std::{env, ffi::OsStr, path::Path, process::Command};

use crate::{Result, output, process};

use super::KernelBuild;

impl KernelBuild {
    pub fn rust_flags(&self) -> Vec<String> {
        let rust_dir = self.dir.join("rust");
        let rustc_cfg = self.dir.join("include/generated/rustc_cfg");
        let mut flags = vec![
            format!("--extern=core={}", rust_dir.join("libcore.rmeta").display()),
            format!("--extern=alloc={}", rust_dir.join("liballoc.rmeta").display()),
            format!(
                "--extern=compiler_builtins={}",
                rust_dir.join("libcompiler_builtins.rmeta").display()
            ),
            format!(
                "--extern=build_error={}",
                rust_dir.join("libbuild_error.rmeta").display()
            ),
            format!(
                "--extern=bindings={}",
                rust_dir.join("libbindings.rmeta").display()
            ),
            format!("--extern=ffi={}", rust_dir.join("libffi.rmeta").display()),
            format!(
                "--extern=pin_init={}",
                rust_dir.join("libpin_init.rmeta").display()
            ),
            format!("--extern=uapi={}", rust_dir.join("libuapi.rmeta").display()),
            format!(
                "--extern=kernel={}",
                rust_dir.join("libkernel.rmeta").display()
            ),
            format!("-Ldependency={}", rust_dir.display()),
            "-Cpanic=abort".into(),
            "-Cembed-bitcode=n".into(),
            "-Clto=n".into(),
            "-Cforce-unwind-tables=n".into(),
            "-Ccodegen-units=1".into(),
            "-Csymbol-mangling-version=v0".into(),
            "-Crelocation-model=static".into(),
            "-Zfunction-sections=n".into(),
            "-Zdwarf-version=5".into(),
            "-Astable_features".into(),
            "-Aunused_features".into(),
            "--check-cfg=cfg(MODULE)".into(),
            "--cfg=MODULE".into(),
            "--check-cfg=cfg(cargo_nok)".into(),
            "--cfg=cargo_nok".into(),
            "--check-cfg=cfg(cargo_nok_ra)".into(),
            format!("@{}", rustc_cfg.display()),
            "-Zallow-features=asm_const,asm_goto,arbitrary_self_types,lint_reasons,offset_of_nested,raw_ref_op,slice_ptr_len,strict_provenance,used_with_arg".into(),
            "-Zcrate-attr=no_std".into(),
            "-Zcrate-attr=feature(asm_const,asm_goto,arbitrary_self_types,lint_reasons,offset_of_nested,raw_ref_op,slice_ptr_len,strict_provenance,used_with_arg)".into(),
            "-Zunstable-options".into(),
        ];
        flags.extend(self.architecture.rust_flags(&self.config));
        flags
    }

    pub fn run_objtool(&self, object: &Path) -> Result<()> {
        if !self.config.enabled("CONFIG_OBJTOOL") {
            return Ok(());
        }

        let mut command = Command::new(self.dir.join("tools/objtool/objtool"));
        command.args(self.config.objtool_args()).arg(object);
        process::run(&mut command)
    }

    pub fn run_modpost(&self, build_dir: &Path, module_order: &Path, symvers: &Path) -> Result<()> {
        let mut command = Command::new(self.dir.join("scripts/mod/modpost"));
        if self.config.enabled("CONFIG_MODULES") {
            command.arg("-M");
        }
        if self.config.enabled("CONFIG_MODVERSIONS") {
            command.arg("-m");
        }
        if self.config.enabled("CONFIG_BASIC_MODVERSIONS") {
            command.arg("-b");
        }
        if self.config.enabled("CONFIG_EXTENDED_MODVERSIONS") {
            command.arg("-x");
        }
        if self.config.enabled("CONFIG_MODULE_SRCVERSION_ALL") {
            command.arg("-a");
        }
        if !self.config.enabled("CONFIG_SECTION_MISMATCH_WARN_ONLY") {
            command.arg("-E");
        }
        if self
            .config
            .enabled("CONFIG_MODULE_ALLOW_MISSING_NAMESPACE_IMPORTS")
        {
            command.arg("-N");
        }

        command
            .arg("-o")
            .arg(symvers.file_name().unwrap())
            .arg("-T")
            .arg(module_order.file_name().unwrap());
        let kernel_symvers = self.dir.join("Module.symvers");
        if kernel_symvers.is_file() {
            command.arg("-i").arg(kernel_symvers).arg("-e");
        }
        command.current_dir(build_dir);
        process::run(&mut command)
    }

    pub fn compile_c(
        &self,
        source: &Path,
        output: &Path,
        basename: &str,
        module_name: &str,
    ) -> Result<()> {
        let source_arch = self.architecture.source_name();
        let include_dirs = [
            format!("arch/{source_arch}/include"),
            format!("arch/{source_arch}/include/generated"),
            "include".into(),
            format!("arch/{source_arch}/include/uapi"),
            format!("arch/{source_arch}/include/generated/uapi"),
            "include/uapi".into(),
            "include/generated/uapi".into(),
        ];

        let mut command = Command::new(env::var_os("CC").unwrap_or_else(|| "gcc".into()));
        command.arg("-nostdinc");
        for include in &include_dirs {
            command.arg(format!("-I{}", self.dir.join(include).display()));
        }

        command
            .arg("-include")
            .arg(self.dir.join("include/linux/compiler-version.h"))
            .arg("-include")
            .arg(self.dir.join("include/linux/kconfig.h"))
            .arg("-include")
            .arg(self.dir.join("include/linux/compiler_types.h"))
            .args([
                "-D__KERNEL__",
                "-DMODULE",
                "-std=gnu11",
                "-fshort-wchar",
                "-funsigned-char",
                "-fno-common",
                "-fno-PIE",
                "-fno-strict-aliasing",
                "-fno-asynchronous-unwind-tables",
                "-fno-jump-tables",
                "-fno-delete-null-pointer-checks",
                "-fno-stack-clash-protection",
                "-fno-strict-overflow",
                "-fno-stack-check",
                "-fms-extensions",
                "-fstrict-flex-arrays=3",
                "-fno-builtin-wcslen",
                "-O2",
                "-g",
                "-gdwarf-5",
            ])
            .args(self.architecture.c_flags(&self.config))
            .arg(format!("-DKBUILD_BASENAME=\"{basename}\""))
            .arg(format!("-DKBUILD_MODNAME=\"{module_name}\""))
            .arg(format!("-D__KBUILD_MODNAME={module_name}"))
            .arg("-c")
            .arg("-o")
            .arg(output)
            .arg(source);
        process::run(&mut command)
    }

    pub fn maybe_generate_btf(&self, output: &Path) -> Result<()> {
        if env::var_os("NOK_BTF").as_deref() != Some(OsStr::new("1")) {
            return Ok(());
        }

        let vmlinux = self.dir.join("vmlinux");
        if !vmlinux.is_file() {
            output::warning(format_args!(
                "skipping BTF because {} is missing",
                vmlinux.display()
            ));
            return Ok(());
        }

        output::status("Generating", format_args!("BTF for {}", output.display()));
        process::run(
            Command::new("sh")
                .arg(self.dir.join("scripts/gen-btf.sh"))
                .arg("--btf_base")
                .arg(vmlinux)
                .arg(output),
        )
    }

    pub fn rust_target(&self) -> &OsStr {
        self.target.as_os_str()
    }
}
