use std::fs::OpenOptions;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::{env, fs};

pub(crate) fn ensure_host_daemon() -> Result<(), String> {
    let socket_path = host_notify_socket_path()?;
    let startup_lock_path = format!("{socket_path}.startup.lock");
    let startup_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(startup_lock_path)
        .map_err(|error| format!("failed to open Host daemon startup lock: {error}"))?;
    startup_lock
        .lock()
        .map_err(|error| format!("failed to lock Host daemon startup: {error}"))?;
    if StdUnixStream::connect(&socket_path).is_ok() {
        return Ok(());
    }
    let _ = fs::remove_file(&socket_path);
    let executable =
        env::current_exe().map_err(|error| format!("failed to find current exe: {error}"))?;
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{socket_path}.log"))
        .map_err(|error| format!("failed to open Host daemon log: {error}"))?;
    let log_file_stderr = log_file
        .try_clone()
        .map_err(|error| format!("failed to duplicate Host daemon log handle: {error}"))?;
    let mut child = Command::new(executable)
        .arg("daemon")
        // Its own process group: closing the pane that started it must not end it.
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_stderr))
        .spawn()
        .map_err(|error| format!("failed to spawn daemon: {error}"))?;
    // The daemon binds its notify socket once it can serve Agent CLI notifications.
    for _ in 0..20 {
        if StdUnixStream::connect(&socket_path).is_ok() {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed to check daemon startup: {error}"))?
        {
            return Err(format!("daemon exited before ready with {status}"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    Err("daemon did not report ready within 2 seconds".to_owned())
}

pub(crate) fn host_notify_socket_path() -> Result<String, String> {
    let user = env::var("USER")
        .or_else(|_| env::var("LOGNAME"))
        .or_else(|_| {
            env::var("HOME").and_then(|home| {
                std::path::Path::new(&home)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .ok_or(env::VarError::NotPresent)
            })
        })
        .map_err(|_| "USER, LOGNAME, and HOME are unavailable for Host identity".to_owned())?;
    let safe_user: String = user
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    Ok(env::temp_dir()
        .join(format!("pingme-{safe_user}.sock"))
        .display()
        .to_string())
}
