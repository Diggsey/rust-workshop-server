use std::{
    fs,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::mpsc,
    thread,
};

use client_id::ClientId;
use log::error;
use ordered_float::NotNan;
use protocol::{Request, Response};
use serde::Deserialize;
use structopt::StructOpt;

use crate::{client_handler::client_connected, output::output_thread, server_state::server_thread};

mod client_handler;
mod client_id;
mod http;
mod output;
mod protocol;
mod server_state;

const TILE_SIZE: usize = 128;
const TILES_X: usize = 8;
const TILES_Y: usize = 6;

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

#[derive(Deserialize)]
struct SceneElement {
    x: f64,
    y: f64,
    z: f64,
    r: f64,
}

#[derive(StructOpt)]
struct Opt {
    scene_filename: PathBuf,
    #[structopt(short, long, default_value = "0.0.0.0:1234")]
    addr: SocketAddr,
}

fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let _ = pretty_env_logger::try_init();
    let opt = Opt::from_args();

    // Wipe the live video directory before starting
    let _ = fs::remove_dir_all("static/livevideo");
    fs::create_dir_all("static/livevideo")?;
    fs::create_dir_all("static/recording")?;

    let scene_reader = csv::Reader::from_path(opt.scene_filename)?;
    let mut scene_elements = scene_reader
        .into_deserialize()
        .collect::<Result<Vec<SceneElement>, _>>()?;
    scene_elements.sort_by_key(|elem| NotNan::new(elem.x).unwrap());

    let listener = TcpListener::bind(opt.addr)?;
    let (client_tx, client_rx) = mpsc::sync_channel(256);
    let (output_tx, output_rx) = mpsc::sync_channel(256);

    thread::spawn(move || output_thread(output_rx).unwrap());
    thread::spawn(move || server_thread(client_rx, output_tx, scene_elements));
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
