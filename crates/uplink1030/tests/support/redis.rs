//! Ephemeral loopback RESP fixture: never connects to or modifies a user's Redis.
use std::{
    io,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout, Duration},
};

#[derive(Clone, Copy)]
pub enum Mode {
    Healthy,
    DisconnectFirst,
    RejectFirst,
    HoldFirst,
    HoldAll,
}
#[derive(Clone, Debug)]
pub struct Seen {
    pub connection: usize,
    pub topic: String,
    pub payload: String,
}
pub struct Server {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub async fn start(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("redis://{}/", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let shared = seen.clone();
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            let mut id = 0;
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let Ok((stream,_))=accepted else {break;};
                        let seen=shared.clone();let connection=id;id+=1;
                        tasks.spawn(async move {let _=serve(stream,mode,connection,seen).await;});
                    },
                    _=tasks.join_next(),if !tasks.is_empty()=>{},
                }
            }
        });
        Self { url, seen, task }
    }
    pub async fn wait_count(&self, n: usize) {
        timeout(Duration::from_secs(5), async {
            while self.seen.lock().unwrap().len() < n {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("missing Redis publication");
    }
}
async fn command(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Vec<Vec<u8>>>> {
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    let n = line
        .strip_prefix('*')
        .and_then(|s| s.trim().parse::<usize>().ok())
        .ok_or_else(|| io::Error::other("invalid array"))?;
    let mut args = Vec::with_capacity(n);
    for _ in 0..n {
        line.clear();
        reader.read_line(&mut line).await?;
        let n = line
            .strip_prefix('$')
            .and_then(|s| s.trim().parse::<usize>().ok())
            .ok_or_else(|| io::Error::other("invalid bulk"))?;
        let mut bytes = vec![0; n];
        reader.read_exact(&mut bytes).await?;
        let mut end = [0; 2];
        reader.read_exact(&mut end).await?;
        if end != *b"\r\n" {
            return Err(io::Error::other("invalid terminator"));
        }
        args.push(bytes);
    }
    Ok(Some(args))
}
async fn serve(
    stream: TcpStream,
    mode: Mode,
    id: usize,
    seen: Arc<Mutex<Vec<Seen>>>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream);
    while let Some(args) = command(&mut reader).await? {
        if args.first().is_some_and(|a| a == b"PUBLISH") {
            assert_eq!(args.len(), 3);
            let first = {
                let mut seen = seen.lock().unwrap();
                let first = seen.is_empty();
                seen.push(Seen {
                    connection: id,
                    topic: String::from_utf8(args[1].clone()).unwrap(),
                    payload: String::from_utf8(args[2].clone()).unwrap(),
                });
                first
            };
            match mode {
                Mode::DisconnectFirst if first => return Ok(()),
                Mode::RejectFirst if first => {
                    reader
                        .get_mut()
                        .write_all(b"-ERR test rejection\r\n")
                        .await?;
                    continue;
                }
                Mode::HoldAll => std::future::pending::<()>().await,
                Mode::HoldFirst if first => std::future::pending::<()>().await,
                _ => {}
            }
            // Zero subscribers still means Redis accepted the event.
            reader.get_mut().write_all(b":0\r\n").await?;
        } else {
            reader.get_mut().write_all(b"+OK\r\n").await?;
        }
    }
    Ok(())
}
