use edit;
use futures::{stream, StreamExt};
use log::*;
use openssh::*;
use std::fs::File;
use std::io;
use std::io::prelude::*;
use std::process::Output;
use std::time::Instant;
use tokio;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

enum AppError {
    SSHError(openssh::Error),
    IOError(std::io::Error),
}

impl From<std::io::Error> for AppError {
    fn from(error: io::Error) -> Self {
        AppError::IOError(error)
    }
}
impl From<openssh::Error> for AppError {
    fn from(error: openssh::Error) -> Self {
        AppError::SSHError(error)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::SSHError(e) => write!(f, "SSH: {}", e),
            AppError::IOError(e) => write!(f, "IO: {}", e),
        }
    }
}

struct CmdRes {
    host: String,
    out: Option<std::process::Output>,
}

async fn run_file(session: &Session, file: &mut File) -> Result<Output, AppError> {
    let mut sftp = session.sftp();
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let remote_file = format!("/tmp/multi_ssh_target_{}", rand::random::<char>());
    let mut w = sftp.write_to(remote_file.to_string()).await?;
    w.write_all(&contents.into_bytes()).await?;
    w.close().await?;

    let cmd = format!("sh {}", remote_file);
    Ok(session.shell(cmd).output().await?)
}

async fn edit_file(session: &Session, file: &str) -> Result<(), AppError> {
    let mut sftp = session.sftp();
    let contents = match sftp.read_from(file).await {
        Ok(mut r) => {
            let mut c = String::new();
            r.read_to_string(&mut c).await?;
            r.close().await?;
            c
        }
        Err(_) => String::new(),
    };

    let edited = edit::edit(contents)?;

    let mut w = sftp.write_to(file).await?;
    w.write_all(&edited.into_bytes()).await?;
    w.close().await?;

    Ok(())
}

async fn run(host: &str, command: &str) -> Result<CmdRes, AppError> {
    debug!("[{}] connecting", host);
    let session = Session::connect(host, KnownHosts::Strict).await?;
    debug!("[{}] connected", host);
    let now = Instant::now();

    let res = if command.starts_with("edit ") {
        edit_file(&session, &command[5..]).await?;
        None
    } else {
        Some(match File::open(command) {
            Ok(mut file) => run_file(&session, &mut file).await?,
            Err(_e) => session.shell(command).output().await?,
        })
    };
    let elapsed = now.elapsed();
    debug!("[{}] exec took: {:.2?}", host, elapsed);
    session.close().await?;

    Ok(CmdRes {
        host: host.to_string(),
        out: res,
    })
}

async fn app(command: &str) {
    let stdin = io::stdin();
    stream::iter(stdin.lock().lines())
        .map(|host| host.unwrap())
        .map(|host| async move { run(&host, command).await })
        .buffer_unordered(500)
        .for_each(|b| async {
            match b {
                Ok(b) => {
                    let host = &b.host;
                    if let Some(out) = b.out {
                        String::from_utf8(out.stderr).map_or((), |o| {
                            for line in o.lines() {
                                println!("[{}] err {}", host, line)
                            }
                        });
                        String::from_utf8(out.stdout).map_or((), |o| {
                            for line in o.lines() {
                                println!("[{}] out {}", host, line)
                            }
                        });
                    }
                }
                Err(e) => error!("Got an error: {}", e),
            }
        })
        .await;
}

fn main() {
    env_logger::Builder::from_default_env()
        .format(move |buf, rec| writeln!(buf, "{}", rec.args()))
        .init();
    let cmd: String = std::env::args().skip(1).collect::<Vec<String>>().join(" ");
    let mut rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(app(&cmd[..]));
}
