use std::sync::mpsc;

pub trait SyncSenderExt<T> {
    fn send_realtime(&self, item: T, name: &str) -> Result<(), mpsc::SendError<T>>;
}

impl<T> SyncSenderExt<T> for mpsc::SyncSender<T> {
    fn send_realtime(&self, item: T, name: &str) -> Result<(), mpsc::SendError<T>> {
        match self.try_send(item) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(item)) => {
                log::warn!("Backpressure: {}", name);
                self.send(item)
            }
            Err(mpsc::TrySendError::Disconnected(item)) => Err(mpsc::SendError(item)),
        }
    }
}
