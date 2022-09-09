use std::{
    io,
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{self, AtomicU64},
        mpsc,
    },
    thread,
};

use client_id::ClientId;
use log::error;
use protocol::{Request, Response};

use crate::{client_handler::client_connected, server_state::server_thread};

mod client_handler;
mod client_id;
mod protocol;
mod server_state;

pub struct ClientEvent {
    from_id: ClientId,
    payload: ClientEventPayload,
}

pub enum ClientEventPayload {
    Connected(mpsc::Sender<ClientCommand>),
    Disconnected,
    Request(Request),
}

pub enum ClientCommand {
    Response(Response),
}

fn main() -> anyhow::Result<()> {
    let _ = pretty_env_logger::try_init();

    let listener = TcpListener::bind("0.0.0.0:1234")?;
    let (tx, rx) = mpsc::sync_channel(256);

    thread::spawn(move || server_thread(rx));

    for stream in listener.incoming() {
        let tx = tx.clone();
        thread::spawn(move || {
            if let Err(e) = client_connected(stream, tx) {
                error!("{}", e);
            }
        });
    }

    println!("Hello, world!");
    Ok(())
}
