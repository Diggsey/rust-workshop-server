use std::{
    collections::{HashMap, VecDeque},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

use rand::{distributions::Uniform, prelude::Distribution, thread_rng};

use crate::{
    client_id::ClientId,
    output::{BlitTileEvent, OutputEvent},
    protocol::{Ray, Request, Response, Scene, Sphere, Vec3},
    ClientCommand, ClientEvent, ClientEventPayload, SceneElement, TILES_X, TILES_Y, TILE_SIZE,
};

struct ClientState {
    name: String,
    tx: mpsc::Sender<ClientCommand>,
}

#[derive(Debug, Copy, Clone)]
pub struct TileAddr {
    pub frame: u64,
    pub x: usize,
    pub y: usize,
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
    requested_at: Instant,
}

struct ServerState {
    rx: mpsc::Receiver<ClientEvent>,
    tx: mpsc::SyncSender<OutputEvent>,
    clients: HashMap<ClientId, ClientState>,
    pending_tiles: VecDeque<TileAddr>,
    in_flight_tiles: VecDeque<InFlightTile>,
    pending_frame: u64,
    current_frame: u64,
    scene: Arc<Scene>,
    all_rays: Vec<Arc<Vec<Ray>>>,
    random_displacements: Vec<Vec3>,
    scene_elements: Vec<SceneElement>,
}

fn generate_ray(x: usize, y: usize) -> Ray {
    let fx = (x as f64) / ((TILES_X * TILE_SIZE) as f64) - 0.5;
    let fy = (y as f64) / ((TILES_Y * TILE_SIZE) as f64) - 0.5;
    let mut direction = Vec3 {
        x: fx * 0.25,
        y: fy * 0.25,
        z: 1.0,
    };
    direction.normalize();
    Ray {
        origin: Vec3 {
            x: 0.0,
            y: 0.0,
            z: -350.0,
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
fn generate_random_displacements(count: usize) -> Vec<Vec3> {
    let mut rng = thread_rng();
    let distr = Uniform::new_inclusive(-1.0, 1.0);
    (0..count)
        .map(|_| Vec3 {
            x: distr.sample(&mut rng),
            y: distr.sample(&mut rng),
            z: distr.sample(&mut rng),
        })
        .collect()
}

impl ServerState {
    fn new(
        rx: mpsc::Receiver<ClientEvent>,
        tx: mpsc::SyncSender<OutputEvent>,
        scene_elements: Vec<SceneElement>,
    ) -> Self {
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
            random_displacements: generate_random_displacements(scene_elements.len()),
            scene_elements,
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
        let angle = (self.current_frame as f64) * 0.05;
        let displacement = 1000.0 / (self.current_frame as f64 + 5.0);
        let (sa, ca) = angle.sin_cos();
        let spheres = self
            .scene_elements
            .iter()
            .enumerate()
            .map(|(i, elem)| {
                let offset = self.random_displacements[i];
                Sphere {
                    center: Vec3 {
                        x: elem.x * ca + elem.z * sa + offset.x * displacement,
                        y: elem.y + offset.y * displacement,
                        z: elem.z * ca - elem.x * sa + offset.z * displacement,
                    },
                    radius: elem.r,
                }
            })
            .collect();
        self.scene = Arc::new(Scene {
            frame: self.current_frame,
            spheres,
        });
    }
    fn disconnect_client(&mut self, client_id: ClientId) {
        self.clients.remove(&client_id);
        self.in_flight_tiles
            .retain(|tile| tile.client_id != client_id);
    }
    fn run(&mut self) {
        loop {
            let res = if let Some(next_tile) = self.in_flight_tiles.front() {
                self.rx
                    .recv_timeout(next_tile.expires.duration_since(Instant::now()))
            } else {
                self.rx
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            let event = match res {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let client_id = self.in_flight_tiles.front().unwrap().client_id;
                    self.disconnect_client(client_id);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match event.payload {
                ClientEventPayload::Connected(tx) => {
                    self.clients.insert(
                        event.from_id,
                        ClientState {
                            tx,
                            name: "Unnamed".into(),
                        },
                    );
                }
                ClientEventPayload::Disconnected => {
                    self.disconnect_client(event.from_id);
                }
                ClientEventPayload::Request(Request::ReserveRays) => {
                    let addr = self.pop_tile_addr();
                    if addr.frame > self.current_frame {
                        self.current_frame = addr.frame;
                        self.regenerate_scene();
                    }
                    if let Some(client) = self.clients.get_mut(&event.from_id) {
                        let now = Instant::now();
                        self.in_flight_tiles.push_back(InFlightTile {
                            client_id: event.from_id,
                            addr,
                            expires: now + Duration::from_secs(5),
                            requested_at: now,
                        });
                        let _ = client
                            .tx
                            .send(ClientCommand::Response(Response::ReserveRays(
                                self.all_rays[addr.rays_index()].clone(),
                                self.scene.clone(),
                            )));
                    }
                }
                ClientEventPayload::Request(Request::SetName(name)) => {
                    if let Some(client) = self.clients.get_mut(&event.from_id) {
                        let _ = client.tx.send(ClientCommand::Response(Response::SetName));
                        client.name = name;
                    }
                }
                ClientEventPayload::Request(Request::SubmitResults(results)) => {
                    if let Some(client) = self.clients.get_mut(&event.from_id) {
                        let _ = client
                            .tx
                            .send(ClientCommand::Response(Response::SubmitResults));
                        if let Some(idx) = self
                            .in_flight_tiles
                            .iter()
                            .position(|x| x.client_id == event.from_id)
                        {
                            let in_flight_tile = self.in_flight_tiles.remove(idx).unwrap();
                            let _ = self.tx.send(OutputEvent::BlitTile(BlitTileEvent {
                                client_id: event.from_id,
                                time: in_flight_tile.requested_at.elapsed().as_secs_f64(),
                                addr: in_flight_tile.addr,
                                name: client.name.clone(),
                                pixels: results
                                    .into_iter()
                                    .map(|result| {
                                        if let Some(color) = result.color {
                                            color
                                        } else if result.hit {
                                            Vec3 {
                                                x: 1.0,
                                                y: 1.0,
                                                z: 1.0,
                                            }
                                        } else {
                                            Vec3 {
                                                x: 0.0,
                                                y: 0.0,
                                                z: 0.0,
                                            }
                                        }
                                    })
                                    .collect(),
                            }));
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn server_thread(
    rx: mpsc::Receiver<ClientEvent>,
    tx: mpsc::SyncSender<OutputEvent>,
    scene_elements: Vec<SceneElement>,
) {
    ServerState::new(rx, tx, scene_elements).run()
}
