use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::mpsc,
    time::Duration,
};

use crate::{
    client_id::ClientId, protocol::Request, ClientCommand, ClientEvent, ClientEventPayload,
};
use anyhow::{anyhow, Context};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

pub struct ClientHandler {
    id: ClientId,
    stream: TcpStream,
    tx: mpsc::SyncSender<ClientEvent>,
    rx: mpsc::Receiver<ClientCommand>,
}

impl ClientHandler {
    fn new(stream: TcpStream, tx: mpsc::SyncSender<ClientEvent>) -> Self {
        let (tx2, rx) = mpsc::channel();
        let res = Self {
            id: ClientId::new(),
            stream,
            tx,
            rx,
        };
        res.emit(ClientEventPayload::Connected(tx2));
        res
    }
    fn emit(&self, payload: ClientEventPayload) {
        let _ = self.tx.send(ClientEvent {
            from_id: self.id,
            payload,
        });
    }
    pub fn run(&mut self) -> anyhow::Result<()> {
        let timeout = Some(Duration::from_secs(1));
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)?;
        self.stream.set_nodelay(true)?;

        let protocol_version = self.stream.read_u32::<BigEndian>()?;
        if protocol_version > 1 {
            return Err(anyhow!("Unknown protocol version: {protocol_version}"));
        }

        log::info!(
            "Client ({:?} - {}) - Connected (protocol: {})",
            self.id,
            self.stream.peer_addr()?,
            protocol_version
        );

        let mut buffer = Vec::new();
        loop {
            let frame_size = self.stream.read_u32::<BigEndian>()? as usize;
            buffer.resize(frame_size, 0);
            self.stream.read_exact(&mut buffer)?;

            let request: Request = match protocol_version {
                0 => serde_json::from_slice(&buffer)?,
                1 => postcard::from_bytes(&buffer)?,
                _ => unreachable!(),
            };

            self.emit(ClientEventPayload::Request(request));

            match self.rx.recv()? {
                ClientCommand::Response(response) => {
                    let vec = match protocol_version {
                        0 => serde_json::to_vec(&response)?,
                        1 => postcard::to_allocvec(&response)?,
                        _ => unreachable!(),
                    };
                    self.stream.write_u32::<BigEndian>(vec.len() as u32)?;
                    self.stream.write_all(&vec)?;
                }
            }
        }
    }
}

impl Drop for ClientHandler {
    fn drop(&mut self) {
        self.emit(ClientEventPayload::Disconnected);
    }
}

pub fn client_connected(
    stream: Result<TcpStream, io::Error>,
    tx: mpsc::SyncSender<ClientEvent>,
) -> anyhow::Result<()> {
    let mut client_handler = ClientHandler::new(stream?, tx);
    let addr = client_handler.stream.peer_addr()?;
    let id = client_handler.id.0;
    client_handler
        .run()
        .with_context(|| format!("Client ({id} - {addr})"))
}
