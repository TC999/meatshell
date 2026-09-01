// Single-instance coordination. All OS entry points (Windows jump list,
// macOS dock menu, Linux desktop action) launch `meatshell --new-window`.
// The first running instance owns a local endpoint under the data dir; later
// `--new-window` launches connect, send "new-window\n" and exit, and the
// primary opens the new window in-process (Chrome-style). Plain relaunches
// never forward: if the endpoint is taken they run as an independent second
// instance.
//
// Transport split: unix uses a unix-domain socket (`ipc.sock`); Windows uses
// a TCP loopback listener on 127.0.0.1 whose port is published in a port
// file (`ipc.port`), because std's Windows unix-socket support is unstable
// (nightly-only, rust-lang/rust#150487). On Windows every `socket_path`
// argument is therefore reinterpreted as the port-file path.

#[cfg(windows)]
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MSG_NEW_WINDOW: &str = "new-window";

/// Payload sent over the single-instance IPC channel.  Format:
/// `<command>\t<path>` where `command` is one of the constants above and
/// `path` is empty for plain new-window and holds the target folder for
/// `--dir`.  The primary parses it and decides whether to `cd` into a
/// directory when it opens the new window.
pub fn build_msg(directory: Option<&str>) -> String {
    match directory {
        Some(dir) => format!("{MSG_NEW_WINDOW}\t{dir}"),
        None => format!("{MSG_NEW_WINDOW}\t"),
    }
}

/// Parse the payload into (`command`, `directory?`).  Unknown commands
/// are ignored so a future version can add commands without breaking older
/// primaries (they'll just see `None` for directory).
pub fn parse_msg(line: &str) -> (&str, Option<&str>) {
    let mut parts = line.splitn(2, '\t');
    let cmd = parts.next().unwrap_or("");
    let dir = parts.next().and_then(|d| if d.is_empty() { None } else { Some(d) });
    (cmd, dir)
}

#[derive(Debug)]
pub enum Instance {
    /// This process owns the endpoint. `listen` accepts forwarded requests.
    Primary { listen: Listener },
    /// Another instance is running; the new-window request was forwarded and
    /// this process should exit with success.
    Forwarded,
}

/// Try to become the primary instance. With `forward`, a live primary
/// receives a new-window request and `Forwarded` is returned; without it a
/// live primary is reported as an error so a plain relaunch runs as its own
/// instance instead of opening a bonus window in the first one. Never
/// panics on IO trouble — callers treat errors as "just run normally".
/// `directory`, when `Some`, is the target folder for the `--dir` verb.
#[cfg(unix)]
pub fn acquire(
    socket_path: &Path,
    forward: bool,
    directory: Option<&str>,
) -> std::io::Result<Instance> {
    if let Ok(listener) = UnixListener::bind(socket_path) {
        return Ok(Instance::Primary {
            listen: Listener { listener },
        });
    }
    match UnixStream::connect(socket_path) {
        Ok(mut stream) => {
            if !forward {
                return Err(std::io::Error::other(
                    "single-instance primary already running",
                ));
            }
            stream.write_all(format!("{}\n", build_msg(directory)).as_bytes())?;
            stream.flush()?;
            Ok(Instance::Forwarded)
        }
        Err(_) => {
            let _ = std::fs::remove_file(socket_path);
            let listener = UnixListener::bind(socket_path)?;
            Ok(Instance::Primary {
                listen: Listener { listener },
            })
        }
    }
}

/// `socket_path` is the port-file path here (see module docs).
#[cfg(windows)]
pub fn acquire(
    socket_path: &Path,
    forward: bool,
    directory: Option<&str>,
) -> std::io::Result<Instance> {
    let port_file = socket_path;
    if let Some(port) = read_port_file(port_file) {
        if forward {
            if forward_tcp(port, directory).is_ok() {
                return Ok(Instance::Forwarded);
            }
        } else if connect_tcp(port).is_ok() {
            return Err(std::io::Error::other(
                "single-instance primary already running",
            ));
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::fs::write(port_file, port.to_string())?;
    if read_port_file(port_file) != Some(port) {
        drop(listener);
        if forward {
            if let Some(winner) = read_port_file(port_file) {
                if forward_tcp(winner, directory).is_ok() {
                    return Ok(Instance::Forwarded);
                }
            }
        }
        return Err(std::io::Error::other("lost single-instance race"));
    }
    Ok(Instance::Primary {
        listen: Listener { listener },
    })
}

#[cfg(windows)]
fn connect_tcp(port: u16) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300))
}

#[cfg(windows)]
fn forward_tcp(port: u16, directory: Option<&str>) -> std::io::Result<Instance> {
    let mut stream = connect_tcp(port)?;
    stream.write_all(format!("{}\n", build_msg(directory)).as_bytes())?;
    stream.flush()?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    match BufReader::new(&stream).lines().next() {
        Some(Ok(line)) if line == "ack" => Ok(Instance::Forwarded),
        _ => Err(std::io::Error::other("no ack from single-instance primary")),
    }
}

#[cfg(windows)]
fn read_port_file(port_file: &Path) -> Option<u16> {
    std::fs::read_to_string(port_file).ok()?.trim().parse().ok()
}

/// Endpoint path inside the per-user data dir: the unix socket on unix, the
/// TCP port file on Windows (see module docs).
pub fn socket_path() -> PathBuf {
    #[cfg(windows)]
    {
        crate::config::data_dir().join("ipc.port")
    }
    #[cfg(not(windows))]
    {
        crate::config::data_dir().join("ipc.sock")
    }
}

#[derive(Debug)]
pub struct Listener {
    #[cfg(unix)]
    listener: UnixListener,
    #[cfg(windows)]
    listener: TcpListener,
}

impl Listener {
    /// Blocks forever, invoking `on_msg` for every complete line received.
    /// Spawn this on its own thread. Every accepted line is acked before
    /// dispatch so forwarders can verify they reached the real primary —
    /// on Windows the port file may stale and point at an unrelated
    /// listener, which would never ack. `on_msg` receives the directory
    /// path the forwarder asked for (or None for a plain new-window).
    pub fn spawn<F: FnMut(Option<String>) + Send + 'static>(self, mut on_msg: F) {
        for mut stream in self.listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            if let Some(Ok(line)) = BufReader::new(&stream).lines().next() {
                let _ = stream.write_all(b"ack\n");
                let _ = stream.flush();
                let (_cmd, dir) = parse_msg(&line);
                on_msg(dir.map(str::to_owned));
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/app/window_management/single_instance.rs"]
mod single_instance_tests;
