use std::{
    fs,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::{atomic::AtomicBool, mpsc, Arc},
    thread,
};

use client_id::ClientId;
use log::error;
use ordered_float::NotNan;
use protocol::{Request, Response};
use serde::Deserialize;
use signal_hook::{consts::TERM_SIGNALS, flag};
use structopt::StructOpt;

use crate::{client_handler::client_connected, output::output_thread, server_state::server_thread};

mod client_handler;
mod client_id;
mod http;
mod output;
mod protocol;
mod server_state;
mod utils;

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
    Connected(mpsc::SyncSender<ClientCommand>),
    Disconnected,
    Request(Request),
}

pub enum ClientCommand {
    Response(Response),
}

#[derive(Deserialize)]
struct SceneElement {
    x: f32,
    y: f32,
    z: f32,
    r: f32,
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

    // Make sure double CTRL+C and similar kills
    let term_now = Arc::new(AtomicBool::new(false));
    for sig in TERM_SIGNALS {
        // When terminated by a second term signal, exit with exit code 1.
        // This will do nothing the first time (because term_now is false).
        flag::register_conditional_shutdown(*sig, 1, Arc::clone(&term_now))?;
        // But this will "arm" the above for the second time, by setting it to true.
        // The order of registering these is important, if you put this one first, it will
        // first arm and then terminate ‒ all in the first round.
        flag::register(*sig, Arc::clone(&term_now))?;
    }

    let scene_reader = csv::Reader::from_path(opt.scene_filename)?;
    let mut scene_elements = scene_reader
        .into_deserialize()
        .collect::<Result<Vec<SceneElement>, _>>()?;
    scene_elements.sort_by_key(|elem| NotNan::new(elem.x).unwrap());

    let listener = TcpListener::bind(opt.addr)?;
    let (client_tx, client_rx) = mpsc::sync_channel(16);
    let (output_tx, output_rx) = mpsc::sync_channel(16);

    thread::spawn(move || output_thread(output_rx, term_now).unwrap());
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
