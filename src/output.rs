use std::{
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Instant,
};

use gst::{
    prelude::{Cast, GstBinExtManual, ObjectExt},
    traits::ElementExt,
    MessageView,
};

use crate::{protocol::Vec3, server_state::TileAddr, TILES_X, TILES_Y, TILE_SIZE};

#[derive(Debug)]
pub enum OutputEvent {
    BlitTile(BlitTileEvent),
}

#[derive(Debug)]
pub struct BlitTileEvent {
    pub addr: TileAddr,
    pub name: String,
    pub pixels: Vec<Vec3>,
}

struct Accumulator {
    data: Vec<u8>,
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
    sink.set_property("location", "static/segment%05d.ts");
    sink.set_property("playlist-location", "static/playlist.m3u8");

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
        data: vec![255; video_info.size()],
    }));
    let acc2 = acc.clone();

    let begin = Instant::now();
    appsrc.set_callbacks(
        // Since our appsrc element operates in pull mode (it asks us to provide data),
        // we add a handler for the need-data callback and provide new data from there.
        // In our case, we told gstreamer that we do 2 frames per second. While the
        // buffers of all elements of the pipeline are still empty, this will be called
        // a couple of times until all of them are filled. After this initial period,
        // this handler will be called (on average) twice per second.
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
            }
        }
    }
    Ok(())
}
