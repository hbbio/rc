//! Small loopback FTP fixture; no external server or account is contacted.
use std::io::{self, BufRead, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct Server {
    pub address: String,
    pub commands: Arc<Mutex<Vec<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
pub const CONTENT: &[u8] = &[b'x'; 180_000];
impl Server {
    pub fn start(stall: bool) -> Self {
        Self::start_mode(stall, false)
    }
    pub fn truncated() -> Self {
        Self::start_mode(false, true)
    }
    fn start_mode(stall: bool, truncate: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let commands = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&commands);
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = serve(stream, &stopped, &recorded, stall, truncate);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            commands,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn serve(
    mut stream: TcpStream,
    stop: &AtomicBool,
    commands: &Mutex<Vec<Vec<u8>>>,
    stall: bool,
    truncate: bool,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut reader = io::BufReader::new(stream.try_clone()?);
    stream.write_all(b"220 fixture\r\n")?;
    let mut data = None;
    while !stop.load(Ordering::Acquire) {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
        let line = line.strip_suffix(b"\r\n").unwrap_or(&line);
        commands.lock().unwrap().push(line.to_vec());
        let end = line
            .iter()
            .position(|byte| *byte == b' ')
            .unwrap_or(line.len());
        match &line[..end] {
            b"USER" => stream.write_all(b"331 password\r\n")?,
            b"PASS" => stream.write_all(b"230 logged in\r\n")?,
            b"PWD" => stream.write_all(b"257 \"/\"\r\n")?,
            b"CWD" | b"TYPE" | b"OPTS" => stream.write_all(b"200 okay\r\n")?,
            b"FEAT" => {
                stream.write_all(b"211-Features\r\n MLST type*;size*;modify*;\r\n211 End\r\n")?
            }
            b"EPSV" => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                listener.set_nonblocking(true)?;
                write!(
                    stream,
                    "229 Entering Extended Passive Mode (|||{}|)\r\n",
                    listener.local_addr()?.port()
                )?;
                data = Some(listener);
            }
            b"SIZE" => write!(stream, "213 {}\r\n", CONTENT.len())?,
            b"MLSD" | b"RETR" => {
                stream.write_all(b"150 data\r\n")?;
                let listener = data.take().expect("passive connection");
                let mut output = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            if stop.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => return Err(error),
                    }
                };
                if &line[..end] == b"MLSD" {
                    write!(
                        output,
                        "type=cdir; .\r\ntype=file;size={};modify=20261029123456;unix.mode=0644;",
                        CONTENT.len()
                    )?;
                    output.write_all(b"  \xff file;type=a\r\n")?;
                } else if stall {
                    while !stop.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(5));
                    }
                } else {
                    output.write_all(if truncate { &CONTENT[..3] } else { CONTENT })?;
                }
                drop(output);
                stream.write_all(b"226 complete\r\n")?;
            }
            b"QUIT" => {
                stream.write_all(b"221 bye\r\n")?;
                break;
            }
            _ => stream.write_all(b"502 unsupported\r\n")?,
        }
    }
    Ok(())
}
