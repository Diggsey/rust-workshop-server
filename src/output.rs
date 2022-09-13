use std::{
    collections::HashMap,
    fs, mem,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Instant,
};

use gio::{traits::FileExt, Cancellable, File, FileCreateFlags, FileOutputStream};
use gst::{
    prelude::{Cast, GstBinExtManual, ObjectExt},
    traits::ElementExt,
    Element, MessageView,
};
use serde::Serialize;

use crate::{
    client_id::ClientId, protocol::Vec3, server_state::TileAddr, TILES_X, TILES_Y, TILE_SIZE,
};

#[derive(Debug)]
pub enum OutputEvent {
    BlitTile(BlitTileEvent),
}

#[derive(Debug)]
pub struct BlitTileEvent {
    pub client_id: ClientId,
    pub addr: TileAddr,
    pub name: String,
    pub pixels: Vec<Vec3>,
    pub time: f64,
}

#[derive(Serialize, Clone)]
struct ClientState {
    current_count: u32,
    total_count: u32,
    average_time: f64,
    name: String,
}

#[derive(Serialize, Clone)]
struct MetaState {
    tiles: Vec<Option<ClientId>>,
    clients: HashMap<ClientId, ClientState>,
    tiles_x: usize,
    tiles_y: usize,
}

#[derive(Serialize, Clone)]
struct MetaBlitTile {
    client_id: ClientId,
    tile: usize,
    time: f64,
    name: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
enum MetaActionPayload {
    Snapshot(MetaState),
    BlitTile(MetaBlitTile),
}

#[derive(Serialize, Clone)]
struct MetaAction {
    ts: u64,
    payload: MetaActionPayload,
}

struct Accumulator {
    data: Vec<u8>,
    meta_state: MetaState,
    meta_actions: Vec<MetaAction>,
    meta_filename: String,
}

const WIDTH: usize = TILES_X * TILE_SIZE;
const HEIGHT: usize = TILES_Y * TILE_SIZE;

pub fn output_thread(rx: mpsc::Receiver<OutputEvent>) -> anyhow::Result<()> {
    gst::init()?;

    let pipeline = gst::Pipeline::new(None);
    let src = gst::ElementFactory::make("appsrc", None)?;
    // let src = gst::ElementFactory::make("videotestsrc", None)?;
    let videoconvert = gst::ElementFactory::make("videoconvert", None)?;
    let encode = gst::ElementFactory::make("x264enc", None)?;
    let caps = gst::ElementFactory::make("capsfilter", None)?;
    let sink = gst::ElementFactory::make("hlssink2", None)?;

    // src.set_property("is-live", true);
    caps.set_property(
        "caps",
        gst::Caps::builder("video/x-h264")
            .field("profile", "baseline")
            .build(),
    );
    sink.set_property("location", "static/livevideo/segment%05d.ts");
    sink.set_property("playlist-location", "static/livevideo/playlist.m3u8");
    sink.set_property("target-duration", 3u32);

    pipeline.add_many(&[&src, &videoconvert, &encode, &caps, &sink])?;
    gst::Element::link_many(&[&src, &videoconvert, &encode, &caps, &sink])?;

    let appsrc = src
        .dynamic_cast::<gst_app::AppSrc>()
        .expect("Source element is expected to be an appsrc!");

    let video_info =
        gst_video::VideoInfo::builder(gst_video::VideoFormat::Bgrx, WIDTH as u32, HEIGHT as u32)
            .fps(gst::Fraction::new(30, 1))
            .build()
            .expect("Failed to create video info");
    let stride = video_info.stride()[0] as usize;
    let offset = video_info.offset()[0] as usize;

    appsrc.set_caps(Some(&video_info.to_caps().unwrap()));
    appsrc.set_format(gst::Format::Time);

    let acc = Arc::new(Mutex::new(Accumulator {
        data: vec![0x40; video_info.size()],
        meta_state: MetaState {
            tiles: vec![None; TILES_X * TILES_Y],
            clients: HashMap::new(),
            tiles_x: TILES_X,
            tiles_y: TILES_Y,
        },
        meta_actions: Vec::new(),
        meta_filename: String::new(),
    }));
    let acc2 = acc.clone();
    let acc3 = acc.clone();

    let begin = Instant::now();
    sink.connect_closure(
        "get-fragment-stream",
        false,
        glib::closure!(move |_elem: &Element, filename: &str| -> FileOutputStream {
            let new_filename = format!("{}.json", filename);
            let (old_actions, old_filename) = {
                let mut acc_guard = acc3.lock().unwrap();
                let mut new_actions = Vec::new();
                new_actions.push(MetaAction {
                    ts: begin.elapsed().as_millis() as u64,
                    payload: MetaActionPayload::Snapshot(acc_guard.meta_state.clone()),
                });
                (
                    mem::replace(&mut acc_guard.meta_actions, new_actions),
                    mem::replace(&mut acc_guard.meta_filename, new_filename),
                )
            };

            if !old_filename.is_empty() {
                fs::write(old_filename, serde_json::to_string(&old_actions).unwrap()).unwrap();
            }

            let file = File::for_path(filename);
            file.create(FileCreateFlags::REPLACE_DESTINATION, Cancellable::NONE)
                .unwrap()
        }),
    );
    sink.connect_closure(
        "delete-fragment",
        false,
        glib::closure!(move |_elem: &Element, filename: &str| {
            let json_filename = format!("{}.json", filename);
            let _ = fs::remove_file(json_filename);
            let _ = fs::remove_file(filename);
        }),
    );

    appsrc.set_callbacks(
        gst_app::AppSrcCallbacks::builder()
            .need_data(move |appsrc, _| {
                let ts = begin.elapsed().as_millis() as u64;
                // Create the buffer that can hold exactly one BGRx frame.
                let mut buffer = gst::Buffer::with_size(video_info.size()).unwrap();
                let buffer_ref = buffer.get_mut().unwrap();
                buffer_ref.set_pts(ts * gst::ClockTime::MSECOND);
                {
                    let acc_guard = acc.lock().unwrap();
                    buffer_ref.copy_from_slice(0, &acc_guard.data).unwrap();
                }

                // appsrc already handles the error here
                let _ = appsrc.push_buffer(buffer);
            })
            .build(),
    );

    pipeline.set_state(gst::State::Playing)?;

    let bus = pipeline.bus().unwrap();

    thread::spawn(move || {
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            match msg.view() {
                MessageView::Eos(..) => break,
                MessageView::Error(err) => eprintln!("{:?}", err),
                _ => {}
            }
        }
    });

    while let Ok(event) = rx.recv() {
        match event {
            OutputEvent::BlitTile(payload) => {
                let mut acc_guard = acc2.lock().unwrap();
                let buffer = &mut *acc_guard.data;
                for y in 0..TILE_SIZE {
                    for x in 0..TILE_SIZE {
                        let i = offset
                            + (payload.addr.y * TILE_SIZE + y) * stride
                            + (payload.addr.x * TILE_SIZE + x) * 4;
                        let j = y * TILE_SIZE + x;
                        buffer[i] = (payload.pixels[j].z * 255.0) as u8;
                        buffer[i + 1] = (payload.pixels[j].y * 255.0) as u8;
                        buffer[i + 2] = (payload.pixels[j].x * 255.0) as u8;
                    }
                }
                let tile = payload.addr.y * TILES_X + payload.addr.x;

                if let Some(old_client_id) = acc_guard.meta_state.tiles[tile] {
                    let mut old_client = acc_guard
                        .meta_state
                        .clients
                        .get_mut(&old_client_id)
                        .unwrap();
                    old_client.current_count -= 1;
                    if old_client.current_count == 0 {
                        acc_guard.meta_state.clients.remove(&old_client_id);
                    }
                }
                acc_guard.meta_state.tiles[tile] = Some(payload.client_id);

                let client = acc_guard
                    .meta_state
                    .clients
                    .entry(payload.client_id)
                    .or_insert_with(|| ClientState {
                        current_count: 0,
                        total_count: 0,
                        average_time: payload.time,
                        name: String::new(),
                    });

                let name_changed = client.name != payload.name;
                if name_changed {
                    client.name = payload.name.clone();
                }
                client.average_time = client.average_time * 0.99 + payload.time * 0.01;
                client.current_count += 1;
                client.total_count += 1;

                acc_guard.meta_actions.push(MetaAction {
                    ts: begin.elapsed().as_millis() as u64,
                    payload: MetaActionPayload::BlitTile(MetaBlitTile {
                        client_id: payload.client_id,
                        tile,
                        time: payload.time,
                        name: if name_changed {
                            Some(payload.name)
                        } else {
                            None
                        },
                    }),
                });
            }
        }
    }
    Ok(())
}