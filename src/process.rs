use std::process::{Command, Stdio};

use crate::Result;

pub fn run(command: &mut Command) -> Result<()> {
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    let printable = format!("{command:?}");
    let status = command.status()?;
    if !status.success() {
        return Err(format!("command failed with {status}: {printable}").into());
    }
    Ok(())
}
