use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{Result, output, process};

pub fn load(path: &Path) -> Result<()> {
    output::status("Loading", format_args!("kernel module {}", path.display()));
    run_privileged("insmod", [path.as_os_str()])
}

pub fn unload(name: &str) -> Result<()> {
    output::status("Unloading", format_args!("kernel module `{name}`"));
    run_privileged("rmmod", [OsStr::new(name)])
}

pub fn is_loaded(name: &str) -> Result<bool> {
    Ok(fs::read_to_string("/proc/modules")?
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .any(|loaded| loaded == name))
}

fn run_privileged<I, S>(program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = find_executable(program)
        .ok_or_else(|| format!("required command `{program}` was not found"))?;
    let args: Vec<OsString> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect();

    if is_root()? {
        return process::run(Command::new(program).args(&args));
    }

    if let Some(sudo) = find_executable("sudo")
        && credential_command(&sudo, ["-v"])?
    {
        return process::run(Command::new(sudo).arg(&program).args(&args));
    }
    if let Some(doas) = find_executable("doas")
        && credential_command(&doas, ["true"])?
    {
        return process::run(Command::new(doas).arg(&program).args(&args));
    }
    if let Some(su) = find_executable("su")
        && credential_command(&su, ["-c", "true"])?
    {
        let command = shell_command(&program, &args)?;
        return process::run(Command::new(su).arg("-c").arg(command));
    }

    Err("could not obtain root privileges with sudo, doas, or su".into())
}

fn credential_command<I, S>(program: &Path, args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Ok(Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?
        .success())
}

fn is_root() -> Result<bool> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        return Err("failed to determine the current user ID".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim() == "0")
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return is_executable(candidate).then(|| candidate.to_owned());
    }

    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .chain(
            ["/usr/sbin", "/usr/bin", "/sbin", "/bin"]
                .into_iter()
                .map(PathBuf::from),
        )
        .map(|dir| dir.join(name))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn shell_command(program: &Path, args: &[OsString]) -> Result<String> {
    let mut command = shell_quote(
        program
            .to_str()
            .ok_or("command path is not valid UTF-8 for su")?,
    );
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(
            arg.to_str()
                .ok_or("command argument is not valid UTF-8 for su")?,
        ));
    }
    Ok(command)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_arguments_for_su_without_shell_injection() {
        assert_eq!(shell_quote("a b'c"), "'a b'\"'\"'c'");
        assert_eq!(
            shell_command(
                Path::new("/usr/bin/insmod"),
                &[OsString::from("/tmp/module name.ko")]
            )
            .unwrap(),
            "'/usr/bin/insmod' '/tmp/module name.ko'"
        );
    }
}
