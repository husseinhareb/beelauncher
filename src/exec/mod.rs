//! Process launching from .desktop Exec fields.
//!
//! Handles field-code stripping per the XDG spec and spawns processes
//! without an intermediate shell.

use std::{
    io,
    os::unix::process::CommandExt,
    process::Command,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("empty exec string")]
    Empty,
    #[error("failed to spawn: {0}")]
    Spawn(#[from] io::Error),
}

/// Launch an application from its Exec field.
///
/// Field codes (%f, %F, %u, %U, %d, %D, %n, %N, %i, %c, %k, %v, %m) are
/// stripped because beelauncher provides no file/URI context.  %% → %.
pub fn launch(exec: &str) -> Result<(), ExecError> {
    let args = parse_exec(exec)?;
    let (program, rest) = args.split_first().ok_or(ExecError::Empty)?;

    let mut cmd = Command::new(program);
    cmd.args(rest);

    // Detach from our process group so the child outlives us.
    // SAFETY: setsid() is async-signal-safe and has no preconditions.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    cmd.spawn()?;
    Ok(())
}

/// Parse an Exec value into a Vec of argument strings.
/// Handles quoting and field-code removal per XDG Desktop Entry spec §6.
fn parse_exec(exec: &str) -> Result<Vec<String>, ExecError> {
    let exec = exec.trim();
    if exec.is_empty() {
        return Err(ExecError::Empty);
    }

    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = exec.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '\\' if in_quotes => {
                // Escape sequences inside quotes.
                match chars.next() {
                    Some('"') => current.push('"'),
                    Some('\\') => current.push('\\'),
                    Some(c) => { current.push('\\'); current.push(c); }
                    None => current.push('\\'),
                }
            }
            '%' => {
                match chars.next() {
                    Some('%') => current.push('%'),
                    // All other field codes are dropped.
                    Some(_) => {}
                    None => {}
                }
            }
            ' ' | '\t' if !in_quotes => {
                let token = current.trim().to_owned();
                if !token.is_empty() {
                    args.push(token);
                }
                current.clear();
            }
            c => current.push(c),
        }
    }

    let token = current.trim().to_owned();
    if !token.is_empty() {
        args.push(token);
    }

    if args.is_empty() {
        return Err(ExecError::Empty);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_command() {
        let args = parse_exec("firefox").unwrap();
        assert_eq!(args, ["firefox"]);
    }

    #[test]
    fn field_codes_stripped() {
        let args = parse_exec("xdg-open %u").unwrap();
        assert_eq!(args, ["xdg-open"]);
    }

    #[test]
    fn double_percent() {
        let args = parse_exec("echo %%").unwrap();
        assert_eq!(args, ["echo", "%"]);
    }

    #[test]
    fn quoted_spaces() {
        let args = parse_exec(r#"env "FOO=bar baz" myapp"#).unwrap();
        assert_eq!(args, ["env", "FOO=bar baz", "myapp"]);
    }
}
