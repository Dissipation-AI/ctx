//! Talking to the server: connect (starting it through launchd if needed), say hello, send one
//! request, read the answer.

use ctx_protocol::config::Paths;
use ctx_protocol::*;
use std::io::{self, IsTerminal};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const LABEL: &str = "com.ctx.server";

pub fn paths() -> Paths {
    Paths::resolve(ctx_server::sys::user_temp_dir(), ctx_server::sys::uid(), &ctx_server::sys::home())
}

pub struct Connection {
    pub stream: UnixStream,
}

/// Why connecting failed, in words for the person.
pub struct NoServer(pub String);

fn hello(kind: ClientKind) -> Hello {
    Hello {
        protocol: PROTOCOL_VERSION,
        client_version: env!("CARGO_PKG_VERSION").into(),
        kind,
        folder: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ctx_server::sys::home()),
        in_tab: std::env::var("CTX_TAB").ok().and_then(|t| t.parse().ok()),
        declared_name: std::env::var("CTX_AGENT_NAME").ok().filter(|s| !s.trim().is_empty()),
        interactive: io::stdin().is_terminal() && io::stdout().is_terminal(),
    }
}

impl Connection {
    /// Connect, starting the server if it isn't running, and handle a version mismatch.
    pub fn open(kind: ClientKind) -> Result<Connection, NoServer> {
        let paths = paths();
        let socket = paths.socket();
        let mut restarted = false;
        loop {
            let stream = match UnixStream::connect(&socket) {
                Ok(s) => s,
                // A sandbox (an agent's, say) that blocks the socket: the server may well be running.
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(NoServer(format!(
                        "Can't reach the ctx server: connecting to {} was denied. If this runs in a sandbox, run it outside.",
                        socket.display()
                    )));
                }
                Err(_) => {
                    start_server(&paths)?;
                    wait_for_socket(&socket, Duration::from_secs(5)).map_err(|_| {
                        NoServer(format!(
                            "The ctx server didn't start (no socket at {}). {}",
                            socket.display(),
                            log_hint(&paths)
                        ))
                    })?
                }
            };
            let mut stream = stream;
            write_message(&mut stream, &hello(kind))
                .map_err(|e| NoServer(format!("Couldn't talk to the ctx server: {e}")))?;
            let welcome: Option<Welcome> =
                read_message(&mut stream).map_err(|e| NoServer(format!("The ctx server didn't answer: {e}")))?;
            match welcome {
                Some(Welcome::Accepted { .. }) => return Ok(Connection { stream }),
                Some(Welcome::VersionMismatch { message, busy_tabs, .. }) => {
                    if busy_tabs.is_empty() && !restarted {
                        // Nothing is running: restart the server to pick up the new version.
                        use std::io::Write;
                        let mut frame = (ctx_server::server::RESTART_MAGIC.len() as u32).to_le_bytes().to_vec();
                        frame.extend_from_slice(ctx_server::server::RESTART_MAGIC);
                        let _ = stream.write_all(&frame);
                        drop(stream);
                        restarted = true;
                        wait_for_restart(&socket);
                        continue;
                    }
                    return Err(NoServer(message));
                }
                None => return Err(NoServer("The ctx server closed the connection.".into())),
            }
        }
    }

    /// Connect only if a server is already running (never starts one).
    pub fn open_quiet() -> Result<Connection, NoServer> {
        let mut stream = UnixStream::connect(paths().socket()).map_err(|e| NoServer(e.to_string()))?;
        write_message(&mut stream, &hello(ClientKind::Cli)).map_err(|e| NoServer(e.to_string()))?;
        match read_message::<Welcome>(&mut stream) {
            Ok(Some(Welcome::Accepted { .. })) => Ok(Connection { stream }),
            _ => Err(NoServer("no server".into())),
        }
    }

    pub fn send(&mut self, env: &Envelope) -> io::Result<()> {
        write_message(&mut self.stream, env)
    }

    pub fn receive(&mut self) -> io::Result<Response> {
        read_message(&mut self.stream)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "The ctx server closed the connection."))
    }

    pub fn request(&mut self, env: &Envelope) -> io::Result<Response> {
        self.send(env)?;
        self.receive()
    }
}

fn log_hint(paths: &Paths) -> String {
    format!("Its log is {}.", paths.server_log().display())
}

fn wait_for_socket(socket: &std::path::Path, limit: Duration) -> io::Result<UnixStream> {
    let start = Instant::now();
    loop {
        if let Ok(s) = UnixStream::connect(socket) {
            return Ok(s);
        }
        if start.elapsed() > limit {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no server"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// After asking for a restart: wait for the old server to go and the new one to listen.
fn wait_for_restart(socket: &std::path::Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
        if let Ok(mut s) = UnixStream::connect(socket)
            && write_message(&mut s, &hello(ClientKind::Cli)).is_ok()
            && let Ok(Some(Welcome::Accepted { .. })) = read_message::<Welcome>(&mut s)
        {
            return;
        }
    }
}

/// The plist that runs `ctx server` as a LaunchAgent.
pub fn plist(exe: &std::path::Path, log: &std::path::Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>server</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
    <key>StandardOutPath</key>
    <string>{}</string>
</dict>
</plist>
"#,
        xml(&exe.display().to_string()),
        xml(&log.display().to_string()),
        xml(&log.display().to_string())
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn plist_path() -> PathBuf {
    PathBuf::from(ctx_server::sys::home()).join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

/// Start the server as a LaunchAgent (never as our own child: it would inherit this terminal's
/// macOS permissions).
fn start_server(paths: &Paths) -> Result<(), NoServer> {
    if Paths::is_custom() {
        return Err(NoServer(format!(
            "No ctx server is running for CTX_HOME={}. Start one there with: ctx server",
            std::env::var("CTX_HOME").unwrap_or_default()
        )));
    }
    let exe = std::env::current_exe().map_err(|e| NoServer(format!("Couldn't find the ctx program: {e}")))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let _ = std::fs::create_dir_all(&paths.data);
    let plist_file = plist_path();
    let wanted = plist(&exe, &paths.server_log());
    let uid = ctx_server::sys::uid();
    let domain = format!("gui/{uid}");
    // Registered already (CTX.app registers its own copy with SMAppService): just start it,
    // rather than replacing that registration with ours.
    let loaded = std::process::Command::new("/bin/launchctl")
        .args(["print", &format!("{domain}/{LABEL}")])
        .output()
        .is_ok_and(|o| o.status.success());
    if loaded && !plist_file.exists() {
        let k = std::process::Command::new("/bin/launchctl").args(["kickstart", &format!("{domain}/{LABEL}")]).output();
        if k.is_ok_and(|o| o.status.success()) {
            return Ok(());
        }
    }
    let current = std::fs::read_to_string(&plist_file).unwrap_or_default();
    if current != wanted {
        if let Some(d) = plist_file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // Replace an older definition (another ctx binary path).
        let _ = std::process::Command::new("/bin/launchctl").args(["bootout", &format!("{domain}/{LABEL}")]).output();
        std::fs::write(&plist_file, wanted)
            .map_err(|e| NoServer(format!("Couldn't write {}: {e}", plist_file.display())))?;
    }
    let out = std::process::Command::new("/bin/launchctl")
        .args(["bootstrap", &domain, &plist_file.display().to_string()])
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        _ => {
            // Already loaded (for example registered by CTX.app): ask launchd to start it.
            let k =
                std::process::Command::new("/bin/launchctl").args(["kickstart", &format!("{domain}/{LABEL}")]).output();
            match k {
                Ok(o) if o.status.success() => Ok(()),
                Ok(o) => Err(NoServer(format!(
                    "launchd couldn't start the ctx server: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))),
                Err(e) => Err(NoServer(format!("Couldn't run launchctl: {e}"))),
            }
        }
    }
}

/// After `ctx restart server`: wait until the new server answers.
pub fn wait_for_new_server() {
    let socket = paths().socket();
    std::thread::sleep(Duration::from_millis(100));
    wait_for_restart(&socket);
}
