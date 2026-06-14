use std::{
    env, fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};

use crate::{Result, kernel::KernelBuild, process};

const PROXY_ENV: &str = "NOK_RA_PROXY";
const REAL_CARGO_ENV: &str = "NOK_RA_REAL_CARGO";
const REAL_CARGO_HOME_ENV: &str = "NOK_RA_REAL_CARGO_HOME";
const KERNEL_SOURCE_ENV: &str = "NOK_KERNEL_SOURCE";
const RUST_ANALYZER_ENV: &str = "NOK_RUST_ANALYZER";

pub fn launch(args: Vec<String>) -> Result<()> {
    let options = LaunchOptions::parse(args)?;
    let kernel = KernelBuild::discover()?;
    let source = discover_kernel_source(&kernel.dir, options.kernel_source)?;
    let rust_analyzer = options
        .rust_analyzer
        .or_else(|| env::var_os(RUST_ANALYZER_ENV).map(PathBuf::from))
        .or_else(|| find_executable("rust-analyzer"))
        .ok_or("could not find the rust-analyzer executable")?;
    let real_cargo = env::var_os("CARGO")
        .map(PathBuf::from)
        .or_else(|| find_executable("cargo"))
        .ok_or("could not find the real Cargo executable")?;
    let real_cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(default_cargo_home)
        .ok_or("could not determine CARGO_HOME")?;
    let modfile = discover_modfile(&real_cargo, &real_cargo_home)?;
    let proxy_home = env::temp_dir().join(format!("cargo-nok-ra-{}", std::process::id()));
    let proxy_bin = proxy_home.join("bin");
    fs::create_dir_all(&proxy_bin)?;
    let proxy_cargo = proxy_bin.join("cargo");
    symlink(env::current_exe()?, &proxy_cargo)?;
    let proxy_path = prepend_path(&proxy_bin)?;

    let status = Command::new(rust_analyzer)
        .args(options.rust_analyzer_args)
        .env(PROXY_ENV, "1")
        .env(REAL_CARGO_ENV, real_cargo)
        .env(REAL_CARGO_HOME_ENV, real_cargo_home)
        .env(KERNEL_SOURCE_ENV, source)
        .env("NOK_KERNEL_DIR", &kernel.dir)
        .env("RUST_MODFILE", modfile)
        .env("CARGO", proxy_cargo)
        .env("CARGO_HOME", &proxy_home)
        .env("PATH", proxy_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let _ = fs::remove_dir_all(&proxy_home);
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rust-analyzer exited with {status}").into())
    }
}

fn discover_modfile(cargo: &Path, cargo_home: &Path) -> Result<PathBuf> {
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .env("CARGO_HOME", cargo_home)
        .output()?;
    if !output.status.success() {
        return Err("cargo metadata failed while determining RUST_MODFILE".into());
    }

    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let workspace_members = metadata["workspace_default_members"]
        .as_array()
        .or_else(|| metadata["workspace_members"].as_array())
        .ok_or("cargo metadata has no workspace members")?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?;
    let mut modfiles = packages
        .iter()
        .filter(|package| workspace_members.contains(&package["id"]))
        .filter_map(|package| {
            let manifest = Path::new(package["manifest_path"].as_str()?);
            let crate_root = manifest.parent()?;
            let target = package["targets"].as_array()?.iter().find(|target| {
                target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "rlib"))
            })?;
            Path::new(target["src_path"].as_str()?)
                .strip_prefix(crate_root)
                .ok()
                .map(Path::to_path_buf)
        })
        .collect::<Vec<_>>();
    modfiles.sort();
    modfiles.dedup();
    match modfiles.as_slice() {
        [modfile] => Ok(modfile.clone()),
        [] => Err("workspace has no rlib module target for RUST_MODFILE".into()),
        _ => Err(
            "workspace has multiple rlib module targets; rust-analyzer requires one module".into(),
        ),
    }
}

fn prepend_path(directory: &Path) -> Result<std::ffi::OsString> {
    let mut paths = vec![directory.to_path_buf()];
    if let Some(path) = env::var_os("PATH") {
        paths.extend(env::split_paths(&path));
    }
    Ok(env::join_paths(paths)?)
}

pub fn proxy(args: Vec<String>) -> Result<()> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return forward_to_cargo(&args);
    };
    match subcommand {
        "metadata" => proxy_metadata(&args),
        "check" => proxy_check(&args),
        _ => forward_to_cargo(&args),
    }
}

struct LaunchOptions {
    kernel_source: Option<PathBuf>,
    rust_analyzer: Option<PathBuf>,
    rust_analyzer_args: Vec<String>,
}

impl LaunchOptions {
    fn parse(args: Vec<String>) -> Result<Self> {
        let mut kernel_source = None;
        let mut rust_analyzer = None;
        let mut rust_analyzer_args = Vec::new();
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--kernel-source" => {
                    kernel_source = Some(PathBuf::from(
                        iter.next().ok_or("missing value for --kernel-source")?,
                    ));
                }
                "--rust-analyzer" => {
                    rust_analyzer = Some(PathBuf::from(
                        iter.next().ok_or("missing value for --rust-analyzer")?,
                    ));
                }
                "--" => {
                    rust_analyzer_args.extend(iter);
                    break;
                }
                _ if arg.starts_with("--kernel-source=") => {
                    kernel_source =
                        Some(PathBuf::from(arg.split_once('=').expect("checked above").1));
                }
                _ if arg.starts_with("--rust-analyzer=") => {
                    rust_analyzer =
                        Some(PathBuf::from(arg.split_once('=').expect("checked above").1));
                }
                _ => rust_analyzer_args.push(arg),
            }
        }
        Ok(Self {
            kernel_source,
            rust_analyzer,
            rust_analyzer_args,
        })
    }
}

fn discover_kernel_source(kbuild: &Path, explicit: Option<PathBuf>) -> Result<PathBuf> {
    let explicit = explicit.or_else(|| env::var_os(KERNEL_SOURCE_ENV).map(PathBuf::from));
    if let Some(source) = explicit {
        return resolve_kernel_source(kbuild, &source);
    }

    let release = fs::read_to_string(kbuild.join("include/config/kernel.release"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    let mut candidates = vec![kbuild.join("source")];
    if !release.is_empty() {
        candidates.push(PathBuf::from("/lib/modules").join(&release).join("source"));
        candidates.push(PathBuf::from("/usr/src").join(format!("linux-{release}")));
    }
    candidates.extend([PathBuf::from("/usr/src/linux"), kbuild.to_owned()]);

    for candidate in candidates {
        if let Ok(source) = resolve_kernel_source(kbuild, &candidate) {
            return Ok(source);
        }
    }

    Err(format!(
        "could not find Linux sources matching {}\n  pass `--kernel-source <path>` or set {KERNEL_SOURCE_ENV}",
        kbuild.display()
    )
    .into())
}

fn resolve_kernel_source(kbuild: &Path, path: &Path) -> Result<PathBuf> {
    if validate_kernel_source(kbuild, path).is_ok() {
        return Ok(path.canonicalize()?);
    }
    if !path.is_dir() {
        return Err(format!("Linux source directory does not exist: {}", path.display()).into());
    }

    let mut matches = Vec::new();
    for entry in fs::read_dir(path)? {
        let candidate = entry?.path();
        if candidate.is_dir() && validate_kernel_source(kbuild, &candidate).is_ok() {
            matches.push(candidate);
        }
    }
    match matches.as_slice() {
        [source] => Ok(source.canonicalize()?),
        [] => {
            validate_kernel_source(kbuild, path)?;
            unreachable!()
        }
        _ => Err(format!(
            "multiple compatible Linux source trees found below {}; pass the exact path",
            path.display()
        )
        .into()),
    }
}

fn validate_kernel_source(kbuild: &Path, source: &Path) -> Result<()> {
    for relative in [
        "Makefile",
        "rust/kernel/lib.rs",
        "rust/macros/lib.rs",
        "scripts/generate_rust_analyzer.py",
    ] {
        if !source.join(relative).is_file() {
            return Err(format!(
                "Linux source artifact is missing: {}",
                source.join(relative).display()
            )
            .into());
        }
    }
    let build_version = kernel_make_version(&kbuild.join("Makefile"))?;
    let source_version = kernel_make_version(&source.join("Makefile"))?;
    if build_version != source_version {
        return Err(format!(
            "Linux source version mismatch\n  Kbuild: {build_version}\n  source: {source_version}"
        )
        .into());
    }
    Ok(())
}

fn kernel_make_version(makefile: &Path) -> Result<String> {
    let contents = fs::read_to_string(makefile)?;
    let value = |name: &str| {
        contents.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.trim())
        })
    };
    Ok(format!(
        "{}.{}.{}{}",
        value("VERSION").ok_or("kernel Makefile has no VERSION")?,
        value("PATCHLEVEL").ok_or("kernel Makefile has no PATCHLEVEL")?,
        value("SUBLEVEL").ok_or("kernel Makefile has no SUBLEVEL")?,
        value("EXTRAVERSION").unwrap_or_default()
    ))
}

fn proxy_metadata(args: &[String]) -> Result<()> {
    let output = real_cargo_command()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()?;
    if !output.status.success() {
        return Err(format!("cargo metadata failed with {}", output.status).into());
    }
    let mut metadata: Value = serde_json::from_slice(&output.stdout)?;
    inject_kernel_crates(&mut metadata)?;
    println!("{}", serde_json::to_string(&metadata)?);
    Ok(())
}

fn inject_kernel_crates(metadata: &mut Value) -> Result<()> {
    let source = PathBuf::from(
        env::var_os(KERNEL_SOURCE_ENV).ok_or("kernel source is missing from RA environment")?,
    );
    let crates = [
        (
            "compiler_builtins",
            "rust/compiler_builtins.rs",
            false,
            &[][..],
        ),
        ("macros", "rust/macros/lib.rs", true, &[][..]),
        (
            "build_error",
            "rust/build_error.rs",
            false,
            &["compiler_builtins"][..],
        ),
        (
            "pin_init_internal",
            "rust/pin-init/internal/src/lib.rs",
            true,
            &[][..],
        ),
        (
            "pin_init",
            "rust/pin-init/src/lib.rs",
            false,
            &["pin_init_internal", "macros"][..],
        ),
        ("ffi", "rust/ffi.rs", false, &["compiler_builtins"][..]),
        (
            "bindings",
            "rust/bindings/lib.rs",
            false,
            &["ffi", "pin_init"][..],
        ),
        ("uapi", "rust/uapi/lib.rs", false, &["ffi", "pin_init"][..]),
        (
            "kernel",
            "rust/kernel/lib.rs",
            false,
            &[
                "macros",
                "build_error",
                "pin_init",
                "ffi",
                "bindings",
                "uapi",
            ][..],
        ),
    ];
    let ids = crates
        .iter()
        .map(|(name, _, _, _)| {
            (
                *name,
                format!("path+file://{}#{name}@0.0.0", source.to_string_lossy()),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();

    let workspace_members = metadata["workspace_members"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let packages = metadata["packages"]
        .as_array_mut()
        .ok_or("cargo metadata has no packages array")?;
    for (name, relative, proc_macro, deps) in crates {
        let id = ids[name].clone();
        let kind = if proc_macro { "proc-macro" } else { "lib" };
        packages.push(json!({
            "name": name,
            "version": "0.0.0",
            "id": id,
            "license": "GPL-2.0",
            "license_file": null,
            "description": "Rust-for-Linux kernel crate",
            "source": null,
            "dependencies": deps.iter().map(|dep| dependency(dep, &ids[dep])).collect::<Vec<_>>(),
            "targets": [{
                "kind": [kind],
                "crate_types": [kind],
                "name": name,
                "src_path": source.join(relative),
                "edition": "2021",
                "doc": true,
                "doctest": false,
                "test": false
            }],
            "features": {},
            "manifest_path": source.join(relative),
            "metadata": null,
            "publish": null,
            "authors": [],
            "categories": [],
            "keywords": [],
            "readme": null,
            "repository": null,
            "homepage": null,
            "documentation": null,
            "edition": "2021",
            "links": null,
            "default_run": null,
            "rust_version": null
        }));
    }

    for member in &workspace_members {
        let Some(member_id) = member.as_str() else {
            continue;
        };
        if let Some(package) = packages
            .iter_mut()
            .find(|package| package["id"].as_str() == Some(member_id))
            && package["targets"].as_array().is_some_and(|targets| {
                targets.iter().any(|target| {
                    target["kind"]
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "rlib"))
                })
            })
        {
            package["dependencies"]
                .as_array_mut()
                .expect("Cargo package dependencies are an array")
                .push(dependency("kernel", &ids["kernel"]));
        }
    }

    if let Some(resolve) = metadata["resolve"].as_object_mut() {
        let nodes = resolve["nodes"]
            .as_array_mut()
            .ok_or("cargo resolve has no nodes array")?;
        for (name, _, _, deps) in crates {
            nodes.push(resolve_node(
                &ids[name],
                deps.iter().map(|dep| (*dep, ids[*dep].as_str())),
            ));
        }
        for member in workspace_members {
            let Some(member_id) = member.as_str() else {
                continue;
            };
            if let Some(node) = nodes
                .iter_mut()
                .find(|node| node["id"].as_str() == Some(member_id))
            {
                node["dependencies"]
                    .as_array_mut()
                    .expect("Cargo node dependencies are an array")
                    .push(Value::String(ids["kernel"].clone()));
                node["deps"]
                    .as_array_mut()
                    .expect("Cargo node deps are an array")
                    .push(json!({
                        "name": "kernel",
                        "pkg": ids["kernel"],
                        "dep_kinds": [{"kind": null, "target": null}]
                    }));
            }
        }
    }
    Ok(())
}

fn dependency(name: &str, id: &str) -> Value {
    json!({
        "name": name,
        "source": null,
        "req": "*",
        "kind": null,
        "rename": null,
        "optional": false,
        "uses_default_features": false,
        "features": [],
        "target": null,
        "registry": null,
        "path": id
    })
}

fn resolve_node<'a>(id: &str, deps: impl Iterator<Item = (&'a str, &'a str)>) -> Value {
    let deps = deps.collect::<Vec<_>>();
    json!({
        "id": id,
        "dependencies": deps.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
        "deps": deps.iter().map(|(name, id)| json!({
            "name": name,
            "pkg": id,
            "dep_kinds": [{"kind": null, "target": null}]
        })).collect::<Vec<_>>(),
        "features": []
    })
}

fn proxy_check(args: &[String]) -> Result<()> {
    emit_kernel_build_outputs(args)?;
    let check_args = normalize_check_args(args);
    let status = Command::new(env::current_exe()?)
        .args(["nok", "check"])
        .args(check_args)
        .env_remove(PROXY_ENV)
        .env("CARGO", real_cargo_path()?)
        .env("CARGO_HOME", real_cargo_home()?)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo nok check failed with {status}").into())
    }
}

fn normalize_check_args(args: &[String]) -> Vec<String> {
    let mut check_args = Vec::new();
    let mut skip_next = false;
    for arg in args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--target" | "--bin" | "--example" | "--test" | "--bench"
        ) {
            skip_next = true;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--compile-time-deps"
                | "--all-targets"
                | "--bins"
                | "--examples"
                | "--tests"
                | "--benches"
        ) || ["--target=", "--bin=", "--example=", "--test=", "--bench="]
            .iter()
            .any(|prefix| arg.starts_with(prefix))
        {
            continue;
        }
        check_args.push(arg.clone());
    }
    if !check_args.iter().any(|arg| arg == "--lib") {
        check_args.push("--lib".into());
    }
    check_args
}

fn emit_kernel_build_outputs(check_args: &[String]) -> Result<()> {
    let source = PathBuf::from(
        env::var_os(KERNEL_SOURCE_ENV).ok_or("kernel source is missing from RA environment")?,
    );
    let kbuild = PathBuf::from(env::var_os("NOK_KERNEL_DIR").ok_or("Kbuild directory is missing")?);
    let id = |name: &str| format!("path+file://{}#{name}@0.0.0", source.to_string_lossy());
    let rustc_cfg = fs::read_to_string(kbuild.join("include/generated/rustc_cfg"))?;
    let cfgs = rustc_cfg
        .lines()
        .filter_map(|line| line.strip_prefix("--cfg="))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    println!(
        "{}",
        json!({
            "reason": "build-script-executed",
            "package_id": id("kernel"),
            "linked_libs": [],
            "linked_paths": [],
            "cfgs": cfgs,
            "env": [["OBJTREE", kbuild]],
            "out_dir": kbuild.join("rust")
        })
    );
    emit_module_build_outputs(check_args, &kbuild, &cfgs)?;
    for (name, source_path, dylib) in [
        (
            "macros",
            source.join("rust/macros/lib.rs"),
            kbuild.join("rust/libmacros.so"),
        ),
        (
            "pin_init_internal",
            source.join("rust/pin-init/internal/src/lib.rs"),
            kbuild.join("rust/libpin_init_internal.so"),
        ),
    ] {
        if dylib.is_file() {
            println!(
                "{}",
                json!({
                    "reason": "compiler-artifact",
                    "package_id": id(name),
                    "manifest_path": source_path,
                    "target": {
                        "kind": ["proc-macro"],
                        "crate_types": ["proc-macro"],
                        "name": name,
                        "src_path": source_path,
                        "edition": "2021",
                        "doc": true,
                        "doctest": false,
                        "test": false
                    },
                    "profile": {
                        "opt_level": "0",
                        "debuginfo": 0,
                        "debug_assertions": false,
                        "overflow_checks": false,
                        "test": false
                    },
                    "features": [],
                    "filenames": [dylib],
                    "executable": null,
                    "fresh": true
                })
            );
        }
    }
    Ok(())
}

fn emit_module_build_outputs(check_args: &[String], kbuild: &Path, cfgs: &[String]) -> Result<()> {
    let mut command = real_cargo_command();
    command.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(manifest) = option_value(check_args, "--manifest-path") {
        command.arg("--manifest-path").arg(manifest);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err("cargo metadata failed while emitting module cfgs".into());
    }

    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let workspace_members = metadata["workspace_members"]
        .as_array()
        .ok_or("cargo metadata has no workspace members")?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?;
    for package in packages
        .iter()
        .filter(|package| workspace_members.contains(&package["id"]))
    {
        let Some(target) = package["targets"].as_array().and_then(|targets| {
            targets.iter().find(|target| {
                target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "rlib"))
            })
        }) else {
            continue;
        };
        let crate_root = Path::new(
            package["manifest_path"]
                .as_str()
                .ok_or("package has no manifest_path")?,
        )
        .parent()
        .ok_or("package manifest has no parent directory")?;
        let modfile = Path::new(
            target["src_path"]
                .as_str()
                .ok_or("rlib target has no src_path")?,
        )
        .strip_prefix(crate_root)
        .map_err(|_| "rlib source is outside the crate root")?;
        let mut module_cfgs = cfgs.to_vec();
        module_cfgs.push("MODULE".into());
        module_cfgs.push("cargo_nok".into());
        module_cfgs.push("cargo_nok_ra".into());
        println!(
            "{}",
            json!({
                "reason": "build-script-executed",
                "package_id": package["id"],
                "linked_libs": [],
                "linked_paths": [],
                "cfgs": module_cfgs,
                "env": [
                    ["OBJTREE", kbuild],
                    ["RUST_MODFILE", modfile]
                ],
                "out_dir": kbuild.join("rust")
            })
        );
    }
    Ok(())
}

fn option_value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == option)
        .map(|pair| pair[1].as_str())
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.strip_prefix(&format!("{option}=")))
        })
}

fn forward_to_cargo(args: &[String]) -> Result<()> {
    process::run(real_cargo_command().args(args))
}

fn real_cargo_command() -> Command {
    let mut command =
        Command::new(real_cargo_path().unwrap_or_else(|_| PathBuf::from("/usr/bin/cargo")));
    command.env_remove(PROXY_ENV);
    if let Ok(home) = real_cargo_home() {
        command.env("CARGO_HOME", home);
    }
    command
}

fn real_cargo_path() -> Result<PathBuf> {
    Ok(PathBuf::from(
        env::var_os(REAL_CARGO_ENV).ok_or("real Cargo path is missing from RA environment")?,
    ))
}

fn real_cargo_home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        env::var_os(REAL_CARGO_HOME_ENV).ok_or("real CARGO_HOME is missing from RA environment")?,
    ))
}

fn default_cargo_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".cargo"))
}

fn find_executable(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_kernel_source() {
        let options = LaunchOptions::parse(vec![
            "--kernel-source=/kernel".into(),
            "--rust-analyzer".into(),
            "/bin/ra".into(),
            "--".into(),
            "--version".into(),
        ])
        .unwrap();
        assert_eq!(options.kernel_source, Some(PathBuf::from("/kernel")));
        assert_eq!(options.rust_analyzer, Some(PathBuf::from("/bin/ra")));
        assert_eq!(options.rust_analyzer_args, ["--version"]);
    }

    #[test]
    fn normalizes_rust_analyzer_check_arguments() {
        assert_eq!(
            normalize_check_args(&[
                "check".into(),
                "--workspace".into(),
                "--all-targets".into(),
                "--target".into(),
                "host-target".into(),
                "--message-format=json".into(),
            ]),
            ["--workspace", "--message-format=json", "--lib",]
        );
    }
}
