use super::{IODriver, IODriverHandle, TOKEN_SIGNAL};

use std::io;

impl IODriverHandle {
    pub(crate) fn register_signal_receiver(&self, receiver: &mut mio::net::UnixStream) -> io::Result<()> {
        self.mioRegistry.register(receiver, TOKEN_SIGNAL, mio::Interest::READABLE)?;
        Ok(())
    }
}

impl IODriver {
    pub(crate) fn consume_signal_ready(&mut self) -> bool {
        let ret = self.signal_ready;
        self.signal_ready = false;
        ret
    }
}
