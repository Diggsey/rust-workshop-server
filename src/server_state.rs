use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{mpsc, Arc},
    time::Instant,
};

use crate::{
    client_id::ClientId,
    protocol::{Ray, Request, Response, Scene, Sphere, Vec3},
    ClientCommand, ClientEvent, ClientEventPayload,
};

struct ClientState {
    tx: mpsc::Sender<ClientCommand>,
}

struct TileAddr {
    frame: u64,
    x: usize,
    y: usize,
}

impl TileAddr {
    fn rays_index(&self) -> usize {
        self.y * TILES_X + self.x
    }
}

struct InFlightTile {
    client_id: ClientId,
    addr: TileAddr,
    expires: Instant,
}

enum OutputEvent {
    BlitTile(TileAddr, Vec<Vec3>),
}

struct ServerState {
    rx: mpsc::Receiver<ClientEvent>,
    tx: mpsc::Sender<OutputEvent>,
    clients: HashMap<ClientId, ClientState>,
    pending_tiles: VecDeque<TileAddr>,
    in_flight_tiles: VecDeque<InFlightTile>,
    pending_frame: u64,
    current_frame: u64,
    scene: Arc<Scene>,
    all_rays: Vec<Arc<Vec<Ray>>>,
}

fn generate_ray(x: usize, y: usize) -> Ray {
    let fx = (x as f64) / ((TILES_X * TILE_SIZE) as f64) - 0.5;
    let fy = (y as f64) / ((TILES_Y * TILE_SIZE) as f64) - 0.5;
    let mut direction = Vec3 {
        x: fx,
        y: fy,
        z: 1.0,
    };
    direction.normalize();
    Ray {
        origin: Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        direction,
    }
}

fn generate_all_rays() -> Vec<Arc<Vec<Ray>>> {
    let mut res = Vec::with_capacity(TILES_X * TILES_Y);
    for ty in 0..TILES_Y {
        for tx in 0..TILES_X {
            let mut rays = Vec::with_capacity(TILE_SIZE * TILE_SIZE);
            for dy in 0..TILE_SIZE {
                for dx in 0..TILE_SIZE {
                    rays.push(generate_ray(tx * TILE_SIZE + dx, ty * TILE_SIZE + dy));
                }
            }
            res.push(Arc::new(rays));
        }
    }
    res
}

const TILE_SIZE: usize = 64;
const TILES_X: usize = 8;
const TILES_Y: usize = 8;

impl ServerState {
    fn new(rx: mpsc::Receiver<ClientEvent>, tx: mpsc::Sender<OutputEvent>) -> Self {
        Self {
            rx,
            tx,
            clients: HashMap::new(),
            pending_tiles: VecDeque::new(),
            in_flight_tiles: VecDeque::new(),
            pending_frame: 1,
            current_frame: 0,
            scene: Default::default(),
            all_rays: generate_all_rays(),
        }
    }
    fn pop_tile_addr(&mut self) -> TileAddr {
        if let Some(addr) = self.pending_tiles.pop_front() {
            addr
        } else {
            for y in 0..TILES_Y {
                for x in 0..TILES_X {
                    self.pending_tiles.push_back(TileAddr {
                        frame: self.pending_frame,
                        x,
                        y,
                    });
                }
            }
            self.pending_frame += 1;
            self.pop_tile_addr()
        }
    }
    fn regenerate_scene(&mut self) {
        let mut spheres = Vec::new();
        let frame_float = (self.current_frame as f64) * 0.01;
        spheres.push(Sphere {
            center: Vec3 {
                x: frame_float.sin() * 10.0,
                y: frame_float.cos() * 10.0,
                z: 0.0,
            },
            radius: 0.5,
        });
        spheres.push(Sphere {
            center: Vec3 {
                x: 2.0,
                y: 3.0,
                z: 5.0,
            },
            radius: frame_float.sin().abs() * 5.0 + 0.1,
        });
        spheres.push(Sphere {
            center: Vec3 {
                x: -2.0,
                y: -1.0,
                z: 3.0,
            },
            radius: 1.0,
        });
        self.scene = Arc::new(Scene {
            frame: self.current_frame,
            spheres,
        });
    }
    fn run(&mut self) {
        while let Ok(event) = self.rx.recv() {
            match event.payload {
                ClientEventPayload::Connected(tx) => {
                    self.clients.insert(event.from_id, ClientState { tx });
                }
                ClientEventPayload::Disconnected => {
                    self.clients.remove(&event.from_id);
                    self.in_flight_tiles
                        .retain(|tile| tile.client_id != event.from_id);
                }
                ClientEventPayload::Request(Request::ReserveRays) => {
                    let addr = self.pop_tile_addr();
                    if addr.frame > self.current_frame {
                        self.current_frame = addr.frame;
                        self.regenerate_scene();
                    }
                    self.clients[&event.from_id]
                        .tx
                        .send(ClientCommand::Response(Response::ReserveRays(
                            self.all_rays[addr.rays_index()].clone(),
                            self.scene.clone(),
                        )));
                }
                ClientEventPayload::Request(Request::SetName(name)) => todo!(),
                ClientEventPayload::Request(Request::SubmitResults(results)) => todo!(),
            }
        }
    }
}

pub fn server_thread(rx: mpsc::Receiver<ClientEvent>) {}
