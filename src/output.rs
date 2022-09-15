use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Write},
    mem,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDateTime, Utc};
use gio::{
    traits::FileExt, Cancellable, File, FileCreateFlags, FileOutputStream, WriteOutputStream,
};
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
    frame_done: bool,
    meta_state: MetaState,
    meta_actions: Vec<MetaAction>,
    meta_filename: String,
}

struct PlaylistWriter {
    filename: String,
    inner: Cursor<Vec<u8>>,
    playlist_state: Arc<Mutex<PlaylistState>>,
}

#[derive(Default)]
struct PlaylistState {
    last_sequence_no: u32,
    total_elapsed: f64,
    next_duration: f64,
}

impl Write for PlaylistWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl Drop for PlaylistWriter {
    fn drop(&mut self) {
        let inner = mem::replace(self.inner.get_mut(), Vec::new());
        self.inner.set_position(0);
        let inner = String::from_utf8(inner).unwrap();
        let mut written_program_date = false;
        let mut sequence_no = 0;
        for line in inner.lines() {
            if !written_program_date {
                if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                    sequence_no = rest.parse().unwrap();
                }
                if let Some(rest) = line.strip_prefix("#EXTINF:") {
                    written_program_date = true;
                    let segment_duration: f64 = rest.strip_suffix(",").unwrap().parse().unwrap();
                    let elapsed = {
                        let mut guard = self.playlist_state.lock().unwrap();
                        if sequence_no > guard.last_sequence_no {
                            guard.last_sequence_no = sequence_no;
                            guard.total_elapsed += guard.next_duration;
                        }
                        guard.next_duration = segment_duration;
                        guard.total_elapsed
                    };
                    let base_datetime =
                        DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(0, 0), Utc);
                    let datetime = base_datetime
                        + chrono::Duration::from_std(Duration::from_secs_f64(elapsed)).unwrap();

                    writeln!(
                        self.inner,
                        "#EXT-X-PROGRAM-DATE-TIME:{}\n",
                        datetime.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    )
                    .unwrap();
                }
            }
            writeln!(self.inner, "{}", line).unwrap();
        }
        fs::write(&self.filename, self.inner.get_ref()).unwrap();
    }
}

const WIDTH: usize = TILES_X * TILE_SIZE;
const HEIGHT: usize = TILES_Y * TILE_SIZE;

pub fn output_thread(rx: mpsc::Receiver<OutputEvent>) -> anyhow::Result<()> {
    gst::init()?;

    let pipeline = gst::Pipeline::new(None);
    let src = gst::ElementFactory::make("appsrc", None)?;
    let videoconvert = gst::ElementFactory::make("videoconvert", None)?;
    let encode = gst::ElementFactory::make("x264enc", None)?;
    let caps = gst::ElementFactory::make("capsfilter", None)?;
    let parse = gst::ElementFactory::make("h264parse", None)?;
    let sink = gst::ElementFactory::make("hlssink2", None)?;

    let file_pipeline = gst::Pipeline::new(None);
    let file_src = gst::ElementFactory::make("appsrc", None)?;
    let file_videoconvert = gst::ElementFactory::make("videoconvert", None)?;
    let file_encode = gst::ElementFactory::make("x264enc", None)?;
    let file_caps = gst::ElementFactory::make("capsfilter", None)?;
    let file_parse = gst::ElementFactory::make("h264parse", None)?;
    // let file_mux = gst::ElementFactory::make("mp4mux", None)?;
    let file_mux = gst::ElementFactory::make("mpegtsmux", None)?;
    let file_sink = gst::ElementFactory::make("filesink", None)?;

    caps.set_property(
        "caps",
        gst::Caps::builder("video/x-h264")
            .field("profile", "baseline")
            .build(),
    );
    sink.set_property("location", "static/livevideo/segment%05d.ts");
    sink.set_property("playlist-location", "static/livevideo/playlist.m3u8");
    sink.set_property("target-duration", 3u32);

    file_encode.set_property("bitrate", 8092u32);
    file_caps.set_property(
        "caps",
        gst::Caps::builder("video/x-h264")
            .field("profile", "high")
            .build(),
    );
    let ts = Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        .replace(":", "-");
    file_sink.set_property("location", format!("static/recording/{ts}.ts"));

    pipeline.add_many(&[&src, &videoconvert, &encode, &caps, &parse, &sink])?;
    file_pipeline.add_many(&[
        &file_src,
        &file_videoconvert,
        &file_encode,
        &file_caps,
        &file_parse,
        &file_mux,
        &file_sink,
    ])?;
    gst::Element::link_many(&[&src, &videoconvert, &encode, &caps, &parse, &sink])?;
    gst::Element::link_many(&[
        &file_src,
        &file_videoconvert,
        &file_encode,
        &file_caps,
        &file_parse,
        &file_mux,
        &file_sink,
    ])?;

    let video_info =
        gst_video::VideoInfo::builder(gst_video::VideoFormat::Bgrx, WIDTH as u32, HEIGHT as u32)
            .fps(gst::Fraction::new(30, 1))
            .build()
            .expect("Failed to create video info");
    let stride = video_info.stride()[0] as usize;
    let offset = video_info.offset()[0] as usize;

    let appsrc = src
        .dynamic_cast::<gst_app::AppSrc>()
        .expect("Source element is expected to be an appsrc!");

    appsrc.set_caps(Some(&video_info.to_caps().unwrap()));
    appsrc.set_format(gst::Format::Time);
    appsrc.set_is_live(true);

    let file_appsrc = file_src
        .dynamic_cast::<gst_app::AppSrc>()
        .expect("Source element is expected to be an appsrc!");

    file_appsrc.set_caps(Some(&video_info.to_caps().unwrap()));
    file_appsrc.set_format(gst::Format::Time);

    let acc = Arc::new(Mutex::new(Accumulator {
        data: vec![0x40; video_info.size()],
        frame_done: false,
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
    let playlist_state = Arc::new(Mutex::new(PlaylistState::default()));

    let begin = Instant::now();
    sink.connect_closure(
        "get-playlist-stream",
        false,
        glib::closure!(
            move |_elem: &Element, filename: &str| -> WriteOutputStream {
                WriteOutputStream::new(PlaylistWriter {
                    filename: filename.into(),
                    inner: Cursor::new(Vec::new()),
                    playlist_state: playlist_state.clone(),
                })
            }
        ),
    );
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
            file.replace(None, false, FileCreateFlags::NONE, Cancellable::NONE)
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

    let mut i = 0;
    appsrc.set_callbacks(
        gst_app::AppSrcCallbacks::builder()
            .need_data(move |appsrc, _| {
                // Create the buffer that can hold exactly one BGRx frame.
                let mut buffer = gst::Buffer::with_size(video_info.size()).unwrap();
                let buffer_ref = buffer.get_mut().unwrap();
                let frame_done = {
                    let mut acc_guard = acc.lock().unwrap();
                    buffer_ref.copy_from_slice(0, &acc_guard.data).unwrap();
                    mem::replace(&mut acc_guard.frame_done, false)
                };
                let ts = begin.elapsed().as_millis() as u64;
                buffer_ref.set_pts(ts * gst::ClockTime::MSECOND);

                if frame_done {
                    let mut buffer = buffer.copy();
                    let buffer_ref = buffer.get_mut().unwrap();
                    buffer_ref.set_pts(Some(i * 33 * gst::ClockTime::MSECOND));
                    i += 1;
                    let _ = file_appsrc.push_buffer(buffer);
                }
                // appsrc already handles the error here
                let _ = appsrc.push_buffer(buffer);
            })
            .build(),
    );

    pipeline.set_state(gst::State::Playing)?;
    file_pipeline.set_state(gst::State::Playing)?;

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

    let file_bus = file_pipeline.bus().unwrap();
    thread::spawn(move || {
        for msg in file_bus.iter_timed(gst::ClockTime::NONE) {
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
                if payload.addr.x == TILES_X - 1 && payload.addr.y == TILES_Y - 1 {
                    acc_guard.frame_done = true;
                }

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
                client.average_time = client.average_time * 0.999 + payload.time * 0.001;
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
