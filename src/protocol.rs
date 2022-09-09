use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub enum Request {
    ReserveRays,
    SetName(String),
    SubmitResults(Vec<Result>),
}

#[derive(Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
    pub fn normalize(&mut self) {
        let scale = 1.0 / self.length();
        self.x *= scale;
        self.y *= scale;
        self.z *= scale;
    }
}

#[derive(Serialize, Deserialize)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

#[derive(Serialize, Deserialize)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Scene {
    pub frame: u64,
    pub spheres: Vec<Sphere>,
}

#[derive(Serialize, Deserialize)]
pub struct Result {
    pub hit: bool,
    #[serde(default)]
    pub color: Option<Vec3>,
}

#[derive(Serialize, Deserialize)]
pub enum Response {
    ReserveRays(Arc<Vec<Ray>>, Arc<Scene>),
    SetName,
    SubmitResults,
}
