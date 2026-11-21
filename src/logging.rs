use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use env_logger::Env;

struct Tee {
    file: Mutex<Option<std::fs::File>>,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        if let Ok(mut guard) = self.file.lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Ok(mut guard) = self.file.lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.flush();
        }
        Ok(())
    }
}

/// Log to stderr and to `<status file>.log`, with the usual `RUST_LOG` levels.
pub fn init(status_path: &Path) {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(crate::status::log_path(status_path))
        .ok();
    let tee = Tee {
        file: Mutex::new(file),
    };
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(tee)))
        .try_init();
}
