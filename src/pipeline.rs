use std::{env, fs, process::Command};

use crate::{Result, cargo::Cargo, cli, kernel::KernelBuild, module, output, process};

pub fn build(args: Vec<String>) -> Result<()> {
    build_module(args)?;
    Ok(())
}

fn build_module(args: Vec<String>) -> Result<(String, std::path::PathBuf)> {
    cli::validate_cargo_args(&args)?;
    let kernel = KernelBuild::discover()?;
    let cargo = Cargo::from_env();
    let package = cargo.package(&args)?;
    let module_name = package.crate_name.replace('-', "_");
    let rlib = cargo.build(&kernel, &args, &package, &module_name)?;
    let artifact_dir = rlib
        .parent()
        .ok_or_else(|| format!("Cargo artifact has no parent: {}", rlib.display()))?;
    let build_dir = artifact_dir.join("nok").join(&module_name);
    fs::create_dir_all(&build_dir)?;

    let module_object = build_dir.join(format!("{module_name}.o"));
    let mod_c = build_dir.join(format!("{module_name}.mod.c"));
    let mod_object = build_dir.join(format!("{module_name}.mod.o"));
    let common_object = build_dir.join("module-common.o");
    let module_manifest = build_dir.join(format!("{module_name}.mod"));
    let module_command = build_dir.join(format!(".{module_name}.o.cmd"));
    let module_order = build_dir.join("modules.order");
    let symvers = build_dir.join("Module.symvers");
    let output = artifact_dir.join(format!("{module_name}.ko"));

    output::status("Linking", format_args!("kernel object for `{module_name}`"));
    process::run(
        Command::new(env::var_os("LD").unwrap_or_else(|| "ld".into()))
            .arg("-r")
            .arg("--whole-archive")
            .arg(&rlib)
            .arg("--no-whole-archive")
            .arg("-o")
            .arg(&module_object),
    )?;
    process::run(
        Command::new(env::var_os("OBJCOPY").unwrap_or_else(|| "objcopy".into()))
            .arg("--remove-section=.rmeta")
            .arg(&module_object),
    )?;
    kernel.run_objtool(&module_object)?;

    fs::write(&module_manifest, format!("./{module_name}.o\n"))?;
    fs::write(
        &module_command,
        format!("source_{}.o := {}\n", module_name, package.source.display()),
    )?;
    fs::write(&module_order, format!("{}.o\n", module_name))?;

    output::status(
        "Processing",
        format_args!("module metadata for `{module_name}`"),
    );
    kernel.run_modpost(&build_dir, &module_order, &symvers)?;
    if !mod_c.is_file() {
        return Err(format!("modpost did not generate {}", mod_c.display()).into());
    }

    output::status(
        "Compiling",
        format_args!("module metadata for `{module_name}`"),
    );
    kernel.compile_c(
        &mod_c,
        &mod_object,
        &format!("{module_name}.mod"),
        &module_name,
    )?;
    kernel.run_objtool(&mod_object)?;

    output::status("Compiling", "common kernel module metadata");
    kernel.compile_c(
        &kernel.dir.join("scripts/module-common.c"),
        &common_object,
        "module_common",
        "module_common",
    )?;
    kernel.run_objtool(&common_object)?;

    output::status("Linking", format_args!("kernel module `{module_name}`"));
    process::run(
        Command::new(env::var_os("LD").unwrap_or_else(|| "ld".into()))
            .arg("-r")
            .arg("-z")
            .arg("noexecstack")
            .arg("--no-warn-rwx-segments")
            .arg("--build-id=sha1")
            .arg("-T")
            .arg(kernel.dir.join("scripts/module.lds"))
            .arg("-o")
            .arg(&output)
            .arg(&module_object)
            .arg(&mod_object)
            .arg(&common_object),
    )?;

    kernel.maybe_generate_btf(&output)?;
    output::status(
        "Finished",
        format_args!("kernel module {}", output.display()),
    );
    Ok((module_name, output))
}

pub fn expand(args: Vec<String>) -> Result<()> {
    cli::validate_cargo_args(&args)?;
    let kernel = KernelBuild::discover()?;
    let cargo = Cargo::from_env();
    let package = cargo.package(&args)?;
    let module_name = package.crate_name.replace('-', "_");
    cargo.expand(&kernel, &args, &module_name)
}

pub fn load(args: Vec<String>) -> Result<()> {
    let (_, module_path) = build_module(args)?;
    module::load(&module_path)
}

pub fn unload(args: Vec<String>) -> Result<()> {
    cli::validate_cargo_args(&args)?;
    let cargo = Cargo::from_env();
    let package = cargo.package(&args)?;
    module::unload(&package.crate_name.replace('-', "_"))
}

pub fn reload(args: Vec<String>) -> Result<()> {
    let (module_name, module_path) = build_module(args)?;
    if module::is_loaded(&module_name)? {
        module::unload(&module_name)?;
    }
    module::load(&module_path)
}
