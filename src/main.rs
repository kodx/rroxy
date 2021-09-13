use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::{broadcast, Mutex},
    time::sleep,
};

#[derive(Debug, Clone)]
enum Command {
    Connect {
        addr: SocketAddr,
    },
    Data {
        data: Vec<u8>,
        from_addr: SocketAddr,
    },
    Connected,
    Disconnect,
}

#[tokio::main]
async fn main() {
    let pub_addr: SocketAddr = match env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string())
        .parse()
    {
        Ok(addr) => addr,
        Err(_) => {
            println!("Public server address and port not set");
            return;
        }
    };

    let connected = Arc::new(Mutex::new(false));
    let (tx, mut _rx) = broadcast::channel(32);
    let timeout = Duration::from_millis(5_000);
    let tx1 = tx.clone();
    let mut rx = tx.subscribe();
    let lock1 = Arc::clone(&connected);
    let lock2 = Arc::clone(&connected);

    let pubserv_task = tokio::spawn(async move {
        loop {
            let mut socket = match TcpStream::connect(pub_addr).await {
                Ok(ok) => ok,
                Err(_) => {
                    println!(
                        "Reconnect to {} in {} seconds",
                        pub_addr.to_string(),
                        timeout.as_secs()
                    );
                    sleep(timeout).await;
                    continue;
                }
            };
            println!("Connected to public server");
            let (reader, mut writer) = socket.split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            let tx = tx.clone();
            let mut rx = tx.subscribe();
            loop {
                tokio::select! {
                    res = reader.read_line(&mut line) => {
                        if res.unwrap() == 0 {
                            break
                        };
                        let lock = lock1.lock().await;
                        if !*lock {
                            if line.starts_with("CONNECT ") && line.ends_with(" HTTP/1.1\r\n") {
                                let conn_addr: SocketAddr = match line
                                .replace("CONNECT ", "")
                                .replace(" HTTP/1.1\r\n", "")
                                .parse() {
                                    Ok(addr) => addr,
                                    Err(_) => {
                                        println!("Invalid data server address and port");
                                        line.clear();
                                        continue;
                                    }
                                };
                                // writer.write_all("HTTP/1.1 200 OK\r\n".as_bytes()).await.unwrap();
                                tx.send(Command::Connect{ addr: conn_addr }).unwrap();
                            }
                        } else {
                            tx.send(Command::Data{ data: line.as_bytes().to_owned(), from_addr: pub_addr }).unwrap();
                        }
                        line.clear();
                    }
                    Ok(cmd) = rx.recv() => {
                        match cmd {
                            Command::Connected => {
                                let mut lock = lock1.lock().await;
                                *lock = true;
                                writer.write_all("HTTP/1.1 200 OK\r\n".as_bytes()).await.unwrap();
                            }
                            Command::Data { data, from_addr } => {
                                if from_addr != pub_addr {
                                    writer.write_all(&data).await.unwrap();
                                }
                            },
                            Command::Disconnect => {
                                println!("Disconnect {}", pub_addr);
                                break;
                            },
                            _ => ()
                        };
                    }
                }
            }
        }
    });

    let manager = tokio::spawn(async move {
        while let Ok(cmd) = rx.recv().await {
            match cmd {
                Command::Connect { addr } => {
                    println!("Recv Connect: {:?}", addr);
                    let lock = lock2.lock().await;
                    if !*lock {
                        let tx = tx1.clone();
                        tokio::spawn(async move {
                            dataserv_connect(addr, tx).await;
                        });
                        // *lock = true;
                    }
                }
                Command::Data { data, from_addr } => {
                    println!(
                        "Recv Data: {:?} | {:?}",
                        String::from_utf8(data).unwrap(),
                        from_addr
                    );
                }
                Command::Disconnect => {
                    println!("Disconnect");
                    let mut lock = lock2.lock().await;
                    *lock = false;
                }
                _ => (),
            }
        }
    });

    manager.await.unwrap();
    pubserv_task.await.unwrap();
}

async fn dataserv_connect(addr: SocketAddr, tx: broadcast::Sender<Command>) {
    let mut socket = match TcpStream::connect(addr).await {
        Ok(ok) => ok,
        Err(_) => {
            println!("Failed to connect to {}", addr.to_string());
            return;
        }
    };
    println!("Connected to data server");
    let conn_addr = socket.peer_addr().unwrap();
    let (reader, mut writer) = socket.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let tx = tx.clone();
    let mut rx = tx.subscribe();
    tx.send(Command::Connected).unwrap();
    loop {
        tokio::select! {
            res = reader.read_line(&mut line) => {
                if res.unwrap() == 0 {
                    break
                };
                tx.send(Command::Data{ data: line.as_bytes().to_owned(), from_addr: conn_addr }).unwrap();
            }
            Ok(cmd) = rx.recv() => {
                if let Command::Data { data, from_addr } = cmd {
                    if from_addr != addr {
                        writer.write_all(&data).await.unwrap();
                    }
                };
            }
        }
    }
    tx.send(Command::Disconnect).unwrap();
}
