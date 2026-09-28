//! Detached catalog worker launch without inheriting the caller's pipe handles.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::process::Command;
use std::ptr;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
};

pub(super) fn spawn(command: &Command) -> io::Result<()> {
    let program = command.get_program();
    let mut application: Vec<u16> = program.encode_wide().collect();
    if application.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in catalog worker executable path",
        ));
    }
    application.push(0);

    // CreateProcessW takes a writable command line. Encode args as Rust's
    // Windows Command does, so spaces and JSON quotes survive CRT parsing.
    let mut command_line = Vec::new();
    append_quoted_arg(&mut command_line, program)?;
    for arg in command.get_args() {
        command_line.push(b' ' as u16);
        append_quoted_arg(&mut command_line, arg)?;
    }
    command_line.push(0);

    // DETACHED_PROCESS gives a console application no parent console or stdio.
    // No STARTF_USESTDHANDLES: that flag requires inheritable handles. Passing
    // FALSE for bInheritHandles is what prevents *other* inheritable handles
    // (including captured output pipes) from leaking into the worker.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // Windows has no zombie state. Closing both parent handles leaves the
    // process running independently, without a blocking reaper thread.
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}

fn append_quoted_arg(command_line: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    command_line.push(QUOTE);
    let mut backslashes = 0;
    for unit in arg.encode_wide() {
        if unit == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NUL in catalog worker argument",
            ));
        }
        if unit == BACKSLASH {
            backslashes += 1;
            continue;
        }
        command_line.extend(std::iter::repeat_n(
            BACKSLASH,
            if unit == QUOTE {
                backslashes * 2 + 1
            } else {
                backslashes
            },
        ));
        backslashes = 0;
        command_line.push(unit);
    }
    command_line.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    command_line.push(QUOTE);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::append_quoted_arg;
    use std::ffi::OsStr;

    #[test]
    fn quotes_worker_paths_and_json_without_losing_backslashes() {
        let mut line = Vec::new();
        append_quoted_arg(&mut line, OsStr::new(r#"C:\project with spaces\.mars\"#)).unwrap();
        assert_eq!(
            String::from_utf16(&line).unwrap(),
            r#""C:\project with spaces\.mars\\""#
        );
        line.clear();
        append_quoted_arg(&mut line, OsStr::new(r#"["anthropic","openai"]"#)).unwrap();
        assert_eq!(
            String::from_utf16(&line).unwrap(),
            r#""[\"anthropic\",\"openai\"]""#
        );
        line.clear();
        append_quoted_arg(&mut line, OsStr::new(r#"a\"b"#)).unwrap();
        assert_eq!(String::from_utf16(&line).unwrap(), r#""a\\\"b""#);
    }
}
