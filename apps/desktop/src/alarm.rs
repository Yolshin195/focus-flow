//! Сигнал «время вышло»: системный звук по кругу, пока человек не
//! отреагирует (как будильник «Часов»).
//!
//! macOS — `NSSound` с `loops`. На Linux и Windows своего звука пока нет:
//! остаётся звук уведомления.

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSString;

    /// Системный звук из `/System/Library/Sounds`.
    const SOUND: &str = "Glass";

    thread_local! {
        static PLAYING: RefCell<Option<Retained<NSSound>>> = const { RefCell::new(None) };
    }

    pub fn set(on: bool) {
        PLAYING.with(|p| {
            let mut p = p.borrow_mut();
            if !on {
                if let Some(sound) = p.take() {
                    sound.stop();
                }
                return;
            }
            if p.is_some() {
                return;
            }
            let Some(sound) = NSSound::soundNamed(&NSString::from_str(SOUND)) else {
                eprintln!("сигнал: нет системного звука «{SOUND}»");
                return;
            };
            sound.setLoops(true);
            if sound.play() {
                *p = Some(sound);
            } else {
                eprintln!("сигнал: звук «{SOUND}» не запустился");
            }
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn set(_on: bool) {}
}

pub use imp::set;
