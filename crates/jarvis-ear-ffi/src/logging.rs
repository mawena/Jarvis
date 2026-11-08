//! Journal `tracing` vers `logcat` (étiquette `JarvisRust`), sans nouvelle crate :
//! `__android_log_write` de `liblog`, déclarée à la main. Hors Android : rien.

use std::sync::OnceLock;

static INIT: OnceLock<()> = OnceLock::new();

/// Installe l'abonné une seule fois ; les appels suivants ne font rien.
pub fn init() {
    INIT.get_or_init(|| {
        #[cfg(target_os = "android")]
        android::install();
    });
}

#[cfg(target_os = "android")]
mod android {
    use std::ffi::{CString, c_char, c_int};
    use std::io::{self, Write};

    use tracing::{Level, Metadata};
    use tracing_subscriber::fmt::MakeWriter;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    // Priorités de `android/log.h`.
    const DEBUG: c_int = 3;
    const INFO: c_int = 4;
    const WARN: c_int = 5;
    const ERROR: c_int = 6;

    const TAG: &std::ffi::CStr = c"JarvisRust";

    pub fn install() {
        // Échec possible seulement si un autre abonné global existe déjà : on le garde.
        let _ = tracing_subscriber::fmt()
            .with_writer(Logcat)
            .with_ansi(false)
            .without_time()
            .with_max_level(Level::INFO)
            .try_init();
    }

    struct Logcat;

    impl<'a> MakeWriter<'a> for Logcat {
        type Writer = Line;

        fn make_writer(&'a self) -> Line {
            Line::new(INFO)
        }

        fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Line {
            Line::new(match *meta.level() {
                Level::ERROR => ERROR,
                Level::WARN => WARN,
                Level::INFO => INFO,
                _ => DEBUG,
            })
        }
    }

    /// Une ligne du journal, envoyée à `logcat` quand `tracing` la lâche.
    struct Line {
        prio: c_int,
        buf: Vec<u8>,
    }

    impl Line {
        fn new(prio: c_int) -> Self {
            Self {
                prio,
                buf: Vec::with_capacity(256),
            }
        }
    }

    impl Write for Line {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Line {
        fn drop(&mut self) {
            while self.buf.last() == Some(&b'\n') {
                self.buf.pop();
            }
            if self.buf.is_empty() {
                return;
            }
            // Un octet nul couperait la ligne : il est remplacé.
            let bytes: Vec<u8> = self
                .buf
                .iter()
                .map(|&b| if b == 0 { b' ' } else { b })
                .collect();
            if let Ok(text) = CString::new(bytes) {
                // SAFETY : deux chaînes C valides, terminées par un octet nul, vivantes
                // pendant l'appel ; `liblog` les copie.
                unsafe {
                    __android_log_write(self.prio, TAG.as_ptr(), text.as_ptr());
                }
            }
        }
    }
}
