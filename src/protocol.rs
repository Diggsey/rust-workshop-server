use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    ReserveRays,
    SubmitResults(Vec<Result>),
    SetName(String),
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
    pub fn normalize(&mut self) {
        let scale = 1.0 / self.length();
        self.x *= scale;
        self.y *= scale;
        self.z *= scale;
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Scene {
    pub frame: u64,
    pub spheres: Vec<Sphere>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Result {
    pub hit: bool,
    #[serde(default)]
    pub color: Option<Vec3>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    ReserveRays(Arc<Vec<Ray>>, Arc<Scene>),
    SubmitResults,
    SetName,
}
