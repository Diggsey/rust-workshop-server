use std::{net::TcpListener, sync::mpsc, thread};

use client_id::ClientId;
use log::error;
use protocol::{Request, Response};

use crate::{client_handler::client_connected, output::output_thread, server_state::server_thread};

mod client_handler;
mod client_id;
mod http;
mod output;
mod protocol;
mod server_state;

const TILE_SIZE: usize = 64;
const TILES_X: usize = 8;
const TILES_Y: usize = 8;

#[derive(Debug)]
pub struct ClientEvent {
    from_id: ClientId,
    payload: ClientEventPayload,
}

#[derive(Debug)]
pub enum ClientEventPayload {
    Connected(mpsc::Sender<ClientCommand>),
    Disconnected,
    Request(Request),
}

pub enum ClientCommand {
    Response(Response),
}

fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let _ = pretty_env_logger::try_init();

    let listener = TcpListener::bind("0.0.0.0:1234")?;
    let (client_tx, client_rx) = mpsc::sync_channel(256);
    let (output_tx, output_rx) = mpsc::sync_channel(256);

    thread::spawn(move || output_thread(output_rx));
    thread::spawn(move || server_thread(client_rx, output_tx));
    thread::spawn(move || http::run_server());

    for stream in listener.incoming() {
        let client_tx = client_tx.clone();
        thread::spawn(move || {
            if let Err(e) = client_connected(stream, client_tx) {
                error!("{:?}", e);
            }
        });
    }

    Ok(())
}
