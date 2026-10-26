//! #2355: `BatchManager::new(0)` must not compile — `max_concurrent` is a
//! `NonZeroUsize`, so a zero argument is rejected by the type system
//! instead of panicking at construction time.

use webfang_core::application::batch::BatchManager;

fn main() {
    let _ = BatchManager::new(0);
}
