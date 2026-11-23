use super::{LIMIT, Result, local_error};
use serde_json::Value;
use std::io::{Read, Write};
use std::process::{Child, Command};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const POLL: Duration = Duration::from_millis(5);

pub(super) fn check(deadline: Instant, cancellation: Option<&CancellationToken>) -> Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err("cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("timeout".into());
    }
    Ok(())
}

pub(super) struct Process {
    child: Child,
    input: Option<platform::Input>,
    output: Option<platform::Output>,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Process {
    pub(super) fn spawn(
        command: &mut Command,
        deadline: Instant,
        cancellation: Option<&CancellationToken>,
    ) -> Result<Self> {
        check(deadline, cancellation)?;
        let (input, stdin, output, stdout) = platform::prepare(deadline, cancellation)?;
        command.stdin(stdin).stdout(stdout);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        check(deadline, cancellation)?;
        let child = command.spawn().map_err(local_error)?;
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null());
        Ok(Self {
            child,
            input: Some(input),
            output: Some(output),
            stop: Arc::new(AtomicBool::new(false)),
            reader: None,
        })
    }

    pub(super) fn close_input(&mut self) {
        self.input.take();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.close_input();
        terminate(&mut self.child);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(unix)]
fn terminate(child: &mut Child) {
    if let Ok(identifier) = i32::try_from(child.id())
        && let Some(group) = rustix::process::Pid::from_raw(identifier)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
fn terminate(child: &mut Child) {
    use std::process::Stdio;
    let cleanup = ["SYSTEMROOT", "WINDIR"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(std::path::PathBuf::from)
        .find(|root| root.is_absolute())
        .and_then(|root| {
            Command::new(root.join("System32").join("taskkill.exe"))
                .args(["/F", "/T", "/PID", &child.id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
        });
    if let Some(mut cleanup) = cleanup {
        let deadline = Instant::now() + Duration::from_secs(1);
        while matches!(cleanup.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        let _ = cleanup.kill();
        let _ = cleanup.wait();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub(super) fn output_channel(
    process: &mut Process,
) -> Result<mpsc::Receiver<std::result::Result<String, String>>> {
    let mut output = process.output.take().ok_or_else(|| local_error("stdout"))?;
    let stop = process.stop.clone();
    let (send, receive) = mpsc::sync_channel(64);
    process.reader = Some(std::thread::spawn(move || {
        let mut pending = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut total = 0u64;
        let mut scanned = 0;
        while !stop.load(Ordering::Relaxed) {
            match output.read(&mut buffer) {
                Ok(0) => {
                    if !pending.is_empty() {
                        send_line(&send, &stop, pending);
                    }
                    break;
                }
                Ok(count) => {
                    total += count as u64;
                    if total > LIMIT {
                        send_output(
                            &send,
                            &stop,
                            Err("Claude Code output exceeded the host limit.".into()),
                        );
                        break;
                    }
                    pending.extend_from_slice(&buffer[..count]);
                    let mut consumed = 0;
                    while let Some(relative) =
                        pending[scanned..].iter().position(|byte| *byte == b'\n')
                    {
                        let end = scanned + relative;
                        if !send_line(&send, &stop, pending[consumed..=end].to_vec()) {
                            return;
                        }
                        consumed = end + 1;
                        scanned = consumed;
                    }
                    pending.drain(..consumed);
                    scanned = pending.len();
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL);
                }
                Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    send_output(
                        &send,
                        &stop,
                        Err("Cannot read the Claude Code output.".into()),
                    );
                    break;
                }
            }
        }
    }));
    Ok(receive)
}

fn send_line(
    send: &mpsc::SyncSender<std::result::Result<String, String>>,
    stop: &AtomicBool,
    bytes: Vec<u8>,
) -> bool {
    send_output(send, stop, String::from_utf8(bytes).map_err(local_error))
}

fn send_output(
    send: &mpsc::SyncSender<std::result::Result<String, String>>,
    stop: &AtomicBool,
    mut value: std::result::Result<String, String>,
) -> bool {
    while !stop.load(Ordering::Relaxed) {
        match send.try_send(value) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Full(returned)) => {
                value = returned;
                std::thread::sleep(POLL);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
        }
    }
    false
}

pub(super) fn write_json(
    process: &mut Process,
    value: &Value,
    deadline: Instant,
    cancellation: Option<&CancellationToken>,
) -> Result<()> {
    check(deadline, cancellation)?;
    let mut bytes = serde_json::to_vec(value).map_err(local_error)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > LIMIT {
        return Err("Claude Code input exceeded the host limit.".into());
    }
    let input = process.input.as_mut().ok_or_else(|| local_error("stdin"))?;
    write_bytes(input, &bytes, deadline, cancellation)
}

fn write_bytes(
    input: &mut impl Write,
    bytes: &[u8],
    deadline: Instant,
    cancellation: Option<&CancellationToken>,
) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        check(deadline, cancellation)?;
        match input.write(&bytes[offset..]) {
            Ok(0) => return Err(local_error("closed stdin")),
            Ok(count) => offset += count,
            Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
            }
            Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => {}
            Err(failure) => return Err(local_error(failure)),
        }
    }
    loop {
        check(deadline, cancellation)?;
        match input.flush() {
            Ok(()) => return check(deadline, cancellation),
            Err(failure) if failure.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
            }
            Err(failure) if failure.kind() == std::io::ErrorKind::Interrupted => {}
            Err(failure) => return Err(local_error(failure)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DeferredWriter;
    impl Write for DeferredWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::WouldBlock.into())
        }
    }

    #[test]
    fn accepted_bytes_do_not_complete_before_the_writer_finishes() {
        for cancelled in [false, true] {
            let cancellation = CancellationToken::new();
            let trigger = cancellation.clone();
            let worker = std::thread::spawn(move || {
                if cancelled {
                    std::thread::sleep(Duration::from_millis(40));
                    trigger.cancel();
                }
            });
            let started = Instant::now();
            let result = write_bytes(
                &mut DeferredWriter,
                b"synthetic",
                started
                    + if cancelled {
                        Duration::from_secs(2)
                    } else {
                        Duration::from_millis(60)
                    },
                Some(&cancellation),
            );
            worker.join().unwrap();
            assert_eq!(
                result,
                Err(if cancelled { "cancelled" } else { "timeout" }.into())
            );
            assert!(started.elapsed() < Duration::from_secs(1));
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::{CancellationToken, Instant, Result, check, local_error};
    use std::io::{PipeReader, PipeWriter};
    use std::process::Stdio;

    pub(super) type Input = PipeWriter;
    pub(super) type Output = PipeReader;

    pub(super) fn prepare(
        deadline: Instant,
        cancellation: Option<&CancellationToken>,
    ) -> Result<(Input, Stdio, Output, Stdio)> {
        check(deadline, cancellation)?;
        let (stdin, input) = std::io::pipe().map_err(local_error)?;
        let (output, stdout) = std::io::pipe().map_err(local_error)?;
        rustix::io::ioctl_fionbio(&input, true).map_err(local_error)?;
        rustix::io::ioctl_fionbio(&output, true).map_err(local_error)?;
        Ok((input, stdin.into(), output, stdout.into()))
    }
}

#[cfg(windows)]
mod platform {
    use super::{CancellationToken, Instant, POLL, Result, check, local_error};
    use std::future::Future;
    use std::io::{Read, Write};
    use std::os::windows::io::AsHandle;
    use std::process::Stdio;
    use std::sync::OnceLock;
    use std::task::{Context, Poll, Waker};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use tokio::runtime::{Builder, Runtime};

    pub(super) struct Input(NamedPipeServer);
    pub(super) struct Output(NamedPipeServer);

    impl Write for Input {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.try_write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.try_write(&[]).map(|_| ())
        }
    }
    impl Drop for Input {
        fn drop(&mut self) {
            let _ = token_station_private_fs::cancel_and_disconnect_named_pipe(self.0.as_handle());
        }
    }
    impl Read for Output {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.0.try_read(bytes)
        }
    }

    fn runtime() -> Result<&'static Runtime> {
        static RUNTIME: OnceLock<std::result::Result<Runtime, std::io::Error>> = OnceLock::new();
        RUNTIME
            .get_or_init(|| {
                Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_io()
                    .build()
            })
            .as_ref()
            .map_err(local_error)
    }

    fn pipe(
        outbound: bool,
        deadline: Instant,
        cancellation: Option<&CancellationToken>,
    ) -> Result<(NamedPipeServer, std::fs::File)> {
        check(deadline, cancellation)?;
        let _entered = runtime()?.enter();
        let name = format!(r"\\.\pipe\token-station-claude-{}", super::super::nonce()?);
        let server = ServerOptions::new()
            .access_inbound(!outbound)
            .access_outbound(outbound)
            .first_pipe_instance(true)
            .max_instances(1)
            .reject_remote_clients(true)
            .create(&name)
            .map_err(local_error)?;
        let client = std::fs::OpenOptions::new()
            .read(outbound)
            .write(!outbound)
            .open(&name)
            .map_err(local_error)?;
        connect(&server, deadline, cancellation)?;
        token_station_private_fs::verify_named_pipe_client_is_current_process(server.as_handle())
            .map_err(local_error)?;
        Ok((server, client))
    }

    fn connect(
        server: &NamedPipeServer,
        deadline: Instant,
        cancellation: Option<&CancellationToken>,
    ) -> Result<()> {
        let mut connection = std::pin::pin!(server.connect());
        let mut context = Context::from_waker(Waker::noop());
        loop {
            check(deadline, cancellation)?;
            match connection.as_mut().poll(&mut context) {
                Poll::Ready(result) => return result.map_err(local_error),
                Poll::Pending => std::thread::sleep(POLL),
            }
        }
    }

    pub(super) fn prepare(
        deadline: Instant,
        cancellation: Option<&CancellationToken>,
    ) -> Result<(Input, Stdio, Output, Stdio)> {
        let (input, stdin) = pipe(true, deadline, cancellation)?;
        let (output, stdout) = pipe(false, deadline, cancellation)?;
        Ok((Input(input), stdin.into(), Output(output), stdout.into()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::process::Command;
        use std::time::Duration;

        const FOREIGN_CLIENT_ENV: &str = "TOKEN_STATION_TEST_CLAUDE_PIPE";

        #[test]
        fn same_parent_named_pipe_setup_succeeds() {
            prepare(Instant::now() + Duration::from_secs(2), None).unwrap();
        }

        #[test]
        fn blocked_write_drop_disconnects_a_retained_client() {
            let (server, mut client) =
                pipe(true, Instant::now() + Duration::from_secs(2), None).unwrap();
            let mut input = Input(server);
            let result = super::super::write_bytes(
                &mut input,
                &vec![b'x'; 1024 * 1024],
                Instant::now() + Duration::from_millis(80),
                None,
            );
            let started = Instant::now();
            drop(input);
            assert!(started.elapsed() < Duration::from_millis(500));
            let mut bytes = [0];
            let received = client.read(&mut bytes);
            assert!(
                matches!(received, Ok(0) | Err(_)),
                "A disconnected client received pending data"
            );
            assert_eq!(result, Err("timeout".into()));
        }

        #[test]
        fn foreign_client() {
            let Some(name) = std::env::var_os(FOREIGN_CLIENT_ENV) else {
                return;
            };
            let mut client = std::fs::OpenOptions::new().read(true).open(name).unwrap();
            let mut bytes = [0];
            let _ = client.read(&mut bytes);
        }

        #[test]
        fn named_pipe_rejects_a_foreign_process_before_data_transfer() {
            let _entered = runtime().unwrap().enter();
            let name = format!(
                r"\\.\pipe\token-station-claude-{}",
                super::super::super::nonce().unwrap()
            );
            let server = ServerOptions::new()
                .access_inbound(false)
                .access_outbound(true)
                .first_pipe_instance(true)
                .max_instances(1)
                .reject_remote_clients(true)
                .create(&name)
                .unwrap();
            let mut client = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "local_subscriptions::claude::process_io::platform::tests::foreign_client",
                    "--nocapture",
                ])
                .env(FOREIGN_CLIENT_ENV, &name)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let connected = connect(&server, Instant::now() + Duration::from_secs(3), None);
            let verified = token_station_private_fs::verify_named_pipe_client_is_current_process(
                server.as_handle(),
            );
            let _ = client.kill();
            client.wait().unwrap();
            connected.unwrap();
            assert_eq!(
                verified.unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }

        #[test]
        fn unconnected_named_pipe_setup_respects_deadline_and_cancellation() {
            let _entered = runtime().unwrap().enter();
            for cancelled in [false, true] {
                let name = format!(
                    r"\\.\pipe\token-station-claude-{}",
                    super::super::super::nonce().unwrap()
                );
                let server = ServerOptions::new()
                    .first_pipe_instance(true)
                    .reject_remote_clients(true)
                    .create(name)
                    .unwrap();
                let cancellation = CancellationToken::new();
                if cancelled {
                    cancellation.cancel();
                }
                let started = Instant::now();
                let failure = connect(
                    &server,
                    started + Duration::from_millis(50),
                    Some(&cancellation),
                )
                .unwrap_err();
                assert_eq!(failure, if cancelled { "cancelled" } else { "timeout" });
                assert!(started.elapsed() < Duration::from_secs(1));
            }
        }
    }
}
